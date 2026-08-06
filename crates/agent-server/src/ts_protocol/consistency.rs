//! 一致性检查：**不依赖 git**（issue 032 原话——仓库尚无提交，git diff 无从谈起）。
//! 每个测试都在本次进程里现导出一份到临时目录，再跟仓库里已经生成的
//! `packages/protocol/` 逐字节比较；不一致的话，多文件/少文件/内容差三种情况
//! 分开报（issue 032 验收原文），最后清理临时目录。

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use super::{
    export_protocol_types, fixtures_path, generated_dir, sample_session_events, write_fixtures,
};

/// `std::env::temp_dir()` 下一个进程内唯一的子目录。pid + 单调计数器 + 纳秒
/// 时间戳三重去重，避免同一次 `cargo test` 并发跑的多个 `#[test]` 撞名字。
/// **这不是红线 1 辖的「derived 读函数」**——这段时间戳只用来造一个不会撞车的
/// 目录名，是测试脚手架，不是 agent-core 原子图的一部分。
fn unique_temp_dir(label: &str) -> PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("系统时钟不至于早于 UNIX_EPOCH")
        .as_nanos();
    std::env::temp_dir().join(format!(
        "agent-server-ts-{label}-{}-{nanos}-{n}",
        std::process::id()
    ))
}

/// 进程内持有临时目录，`Drop` 时删掉——哪怕测试 `assert!` panic 了，栈展开阶段
/// 也会跑到这里，临时目录不会遗留（issue 032「收尾清理临时目录」原话）。
struct TempDirGuard(PathBuf);

impl Drop for TempDirGuard {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// 递归收集 `root` 下全部文件的相对路径。`root` 不存在（比如还没跑过一次生成）
/// 就当空集——调用方会把这解读成「全部文件都少了」，报错信息是对的。
fn relative_files(root: &Path) -> BTreeSet<PathBuf> {
    fn walk(dir: &Path, root: &Path, out: &mut BTreeSet<PathBuf>) {
        let Ok(entries) = fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                walk(&path, root, out);
            } else if let Ok(rel) = path.strip_prefix(root) {
                out.insert(rel.to_path_buf());
            }
        }
    }
    let mut out = BTreeSet::new();
    walk(root, root, &mut out);
    out
}

/// 两棵目录树逐文件字节比较，返回人类可读的差异列表——空列表即一致。三种情况
/// 分开报（issue 032 验收原文）：`fresh`（本次现生成的）有而 `committed`（仓库里
/// 已经生成的）没有——忘了重新生成；`committed` 有而 `fresh` 没有——该删的生成物
/// 还留着；两边都有但内容不同——协议改了但生成物没跟上。
fn diff_dirs(fresh: &Path, committed: &Path) -> Vec<String> {
    let fresh_files = relative_files(fresh);
    let committed_files = relative_files(committed);
    let mut diffs = Vec::new();

    for missing in fresh_files.difference(&committed_files) {
        diffs.push(format!(
            "缺文件：{}（重新生成过，但 packages/protocol/ 里没有）",
            missing.display()
        ));
    }
    for stale in committed_files.difference(&fresh_files) {
        diffs.push(format!(
            "多文件：{}（packages/protocol/ 里有，但重新生成不会产出它了）",
            stale.display()
        ));
    }
    for shared in fresh_files.intersection(&committed_files) {
        let fresh_bytes = fs::read(fresh.join(shared)).expect("fresh 文件刚枚举过，读得到");
        let committed_bytes =
            fs::read(committed.join(shared)).expect("committed 文件刚枚举过，读得到");
        if fresh_bytes != committed_bytes {
            diffs.push(format!("内容不一致：{}", shared.display()));
        }
    }

    diffs
}

const REGEN_HINT: &str =
    "运行 `cargo run -p agent-server --features ts --example gen_protocol_ts` 重新生成";

#[test]
fn generated_ts_matches_committed_snapshot() {
    let temp = TempDirGuard(unique_temp_dir("generated"));
    export_protocol_types(&temp.0).expect("导出协议类型不应该失败");

    let diffs = diff_dirs(&temp.0, &generated_dir());
    assert!(
        diffs.is_empty(),
        "packages/protocol/src/generated/ 跟 Rust 源不一致，{REGEN_HINT}：\n{}",
        diffs.join("\n")
    );
}

#[test]
fn fixtures_json_matches_committed_snapshot() {
    let temp = TempDirGuard(unique_temp_dir("fixtures"));
    let fresh_path = temp.0.join("events.json");
    write_fixtures(&fresh_path).expect("写 fixtures 不应该失败");

    // fixtures 只有一个文件，复用目录 diff：临时目录只放这一个文件，
    // `committed` 那边指到 fixtures.json 的父目录，逻辑跟 generated/ 完全一样,
    // 三种情况（多/少/内容差）报法也一致，不用另开一套比较代码。
    let committed_dir = fixtures_path()
        .parent()
        .expect("fixtures_path 总有父目录")
        .to_path_buf();
    let diffs = diff_dirs(&temp.0, &committed_dir);
    assert!(
        diffs.is_empty(),
        "packages/protocol/fixtures/ 跟 Rust 源不一致，{REGEN_HINT}：\n{}",
        diffs.join("\n")
    );
}

/// 穷举覆盖的直接实检（issue 032 验收原文「`SessionEvent` 全部变体在 fixtures
/// 里各至少一个样本」）。`cast_sample` 的穷举 match 保证「变体存在就必须处理」，
/// 但保证不了「骨架数组本身没有手抖漏一个、错重一个」——这条测试补的正是这个
/// 缺口：17 个变体、17 个样本、互不相同（048 加了 `AgentTree`，054 加了
/// `OrphanedChild`）。变体数变了，先确认 `fixtures::cast_sample` 也跟着改了，
/// 再改这里的 `17`。
#[test]
fn sample_events_cover_every_variant_at_least_once() {
    let samples = sample_session_events();
    assert_eq!(
        samples.len(),
        17,
        "SessionEvent 目前有 17 个变体，样本数应该跟它一一对应"
    );

    let kinds: BTreeSet<&'static str> = samples.iter().map(session_event_kind).collect();
    assert_eq!(
        kinds.len(),
        17,
        "样本里有重复变体，说明漏了另一个——样本种类：{kinds:?}"
    );
}

/// 给样本判别一个 `&'static str`，只给上面那条覆盖率测试用：判断「17 个样本是
/// 不是 17 种不同的变体」而不是「凑巧 17 条但有重复」。穷举、无 `_`——新增变体
/// 这里也编译不过，第三处强制更新点。
fn session_event_kind(ev: &crate::SessionEvent) -> &'static str {
    use crate::SessionEvent::*;
    match ev {
        TextDelta(_) => "TextDelta",
        ThinkingDelta(_) => "ThinkingDelta",
        ToolCallStarted { .. } => "ToolCallStarted",
        PreflightDriftAlert(_) => "PreflightDriftAlert",
        TransportTrouble(_) => "TransportTrouble",
        ToolExecuting { .. } => "ToolExecuting",
        ToolExecuted { .. } => "ToolExecuted",
        TurnGuard { .. } => "TurnGuard",
        Notice(_) => "Notice",
        Undo(_) => "Undo",
        Redo(_) => "Redo",
        Lagged { .. } => "Lagged",
        SessionDied { .. } => "SessionDied",
        Gap { .. } => "Gap",
        AgentTree(_) => "AgentTree",
        OrphanedChild { .. } => "OrphanedChild",
        TransientSourceFailure(_) => "TransientSourceFailure",
    }
}
