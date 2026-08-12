//! `ext:stats`——第一个**真扩展包**（149，教材见 docs/EXTENSIONS.md §「写你的第一个
//! 扩展包」）。这个文件管「包长什么样、怎么装、钩子往哪写」；正文渲染在
//! [`crate::ext_stats_report`]（一个文件一件事）。
//!
//! 包里两条，正好对应扩展能力面的两条路（EXTENSIONS.md §一）：
//!
//! | 条目 | 谁发起 | 干什么 |
//! |---|---|---|
//! | `ext:stats/report`（截获式，`Pure`） | 模型自主调 | 读账本，回一段「这个会话至今干了什么」 |
//! | `ext:stats/audit`（`TurnEnd` timed） | runtime，每个完成轮 | 往 `<session>.audit.log` 追加一行 |
//!
//! # 落点为什么在 `agent-cli` 里、为什么是开关不是 feature
//!
//! 149 的原文给了「feature 门」和「`--ext-stats` 开关」两个选项，取**开关**：
//! feature 门要 `cargo build --features` 才切得动，而这条 issue 的验收第 4 条是
//! 「不装包的会话逐字节零变化」——同一个二进制、跑两次、一次带开关一次不带，是这句
//! 话最直接的证据；feature 门下这两次是两个二进制，「零变化」就得靠比对两份构建产物
//! 说话，反而更绕。不新开 crate 同样照 issue：第一个扩展包先证明接缝。
//!
//! # `Pure` 的举证（EXTENSIONS.md §可逆性：给 `Pure` 的举证责任在包作者）
//!
//! `report` 三条都成立：**纯读**（[`crate::ext_stats_report::render`] 只收 `&Session`，
//! 一条 command 都不发）、**不落 entry**、**没有需要补偿的动作**。它确实有一处副作用
//! ——把这次读到的数字记进 [`Ledger`]，但那是**宿主进程内存里的一格**，既不进状态、
//! 也不改模型看得见的世界；重放一次 `report`（`Reversibility::is_replayable`）算出的
//! 是同一份数字，世界不变。
//!
//! # 交界发现（149 的主要产出之一，150 的输入）：**`TurnEnd` 钩子看不见 `Session`**
//!
//! `TimedRun` 的签名是 `Fn(&ToolTable, &Value) -> Result<Arc<str>, Arc<str>>`（133 的
//! v1 边界，`crate::tool_table` 的 `timed` 子模块解释了为什么不给 async/effect/epoch）
//! ——**它拿不到 `Session`**。于是「每轮把 entry 数写进审计文件」这句需求在今天的机制
//! 上做不到「钩子自己去读账本」：钩子只知道**自己被调过几次**（轮序号），账本数字得
//! 由这个包的另一半（截获式 `report`，它有 `&mut Session`）经 [`Ledger`] 递过来。
//!
//! 所以审计行如实标注这份数字**是哪一轮观测到的**（`seen_at=`）：模型没调 report 的
//! 轮，行照出，数字标 `-` 或停在上一次观测——审计文件宁可承认自己不知道，也不能编一个
//! 看起来很新的数字。这不是绕过机制的临时手法，是机制今天的形状；要让钩子直接读状态，
//! 那是 [150](../../../docs/issues/150-derived-extension-decision.md) 要拍的事
//! （「触发 hook 与 TurnEnd 的关系」正是它列的产出之一）。

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use agent_core::{AgentId, Reversibility, Session, ToolSpec};
use agent_runtime::{
    CallTiming, ExtensionPack, PendingInterceptors, SessionToolFn, TimedRun, ToolTable,
};
use serde_json::{Value, json};

use crate::ext_stats_report::{self, Counts};

/// 包名。进每条工具的全名（`ext:<pack>/<tool>`），装配期硬闸按它逐字校验。
pub const PACK: &str = "stats";
/// 截获式读工具的全名。
pub const REPORT_TOOL: &str = "ext:stats/report";
/// `TurnEnd` 钩子的全名。**不进 prompt**（timed 区对 `specs()` 不可见），但它跟
/// specs 区共用同一个名字空间，所以照样吃 `ext:` 前缀强制。
pub const AUDIT_TOOL: &str = "ext:stats/audit";

/// 命令行开关。**缺省关**——不给 `--ext-stats` 的会话一个字节都不变（验收 4）。
///
/// 收参数而不是自己读 `std::env::args()`，同 [`crate::session_path::resolve`]：
/// 测试要能喂一份夹具参数。
pub fn enabled(args: &[String]) -> bool {
    args.iter().any(|arg| arg == "--ext-stats")
}

/// 审计文件放哪：**会话文件旁**，路径整体追加 `.audit.log`
/// （`a/b/s.jsonl` → `a/b/s.jsonl.audit.log`）。
///
/// 追加而不是 `with_extension`：`with_extension("audit.log")` 会把 `s.jsonl` 换成
/// `s.audit.log`，那个名字可能撞上另一个会话文件；整体追加则永远撞不了，而且一眼看得出
/// 它属于哪份会话。临时会话（没有 `--session`）没有落点 → `None`，钩子照跑但不写盘。
pub fn audit_path(session_file: Option<&Path>) -> Option<PathBuf> {
    let path = session_file?;
    let mut name = path.as_os_str().to_os_string();
    name.push(".audit.log");
    Some(PathBuf::from(name))
}

/// 装配入口：**`on` 为假时原样把表还回去**，`with_extension` 一次都不调。
///
/// 「零变化」因此是结构性的，不是「装了但装了个空包」——空包也会往
/// `host_reversibility` 里插键、也会造一个必须 install 的 [`PendingInterceptors`]。
/// 返回的第二半是 `None`（没装）或 `Some`（装了，**调用方必须 install**，忘了会在
/// `Drop` 里 `debug_assert!` 炸，见 EXTENSIONS.md §防呆）。
pub fn install(
    table: ToolTable,
    on: bool,
    session_file: Option<&Path>,
    note: &mut dyn FnMut(&str),
) -> (ToolTable, Option<PendingInterceptors>) {
    if !on {
        note("关（默认）。工具表逐字节与不装扩展时相同。");
        return (table, None);
    }
    let audit = audit_path(session_file);
    match &audit {
        Some(p) => note(&format!("开。审计文件={}", p.display())),
        None => note("开。**没有会话文件**：审计钩子照跑但无处落盘（用 --session 指定）。"),
    }
    let ledger = Ledger::new(audit);
    let (table, pending) = table.with_extension(pack(ledger));
    (table, Some(pending))
}

/// 组包本身。条目顺序 = 这里写死的 push 顺序（红线 11，包内不排序）。
pub fn pack(ledger: Arc<Ledger>) -> ExtensionPack {
    ExtensionPack::new(PACK)
        .with_tool(
            report_spec(),
            Reversibility::Pure,
            report_run(Arc::clone(&ledger)),
        )
        .with_timed(audit_spec(), CallTiming::TurnEnd, audit_run(ledger))
}

/// 喂给模型的声明。**不带任何会变的字节**（红线 11：描述进 prompt，每一轮都在）。
pub fn report_spec() -> ToolSpec {
    ToolSpec {
        name: Arc::from(REPORT_TOOL),
        description: Arc::from(
            "看一眼这个会话到目前为止的账本汇总：开过几轮、账本上有多少条 entry、\
             你这一支有几个 agent、工具调用了多少次，外加 entry 的种类分布和你的\
             子 agent 列表。**当场返回，不阻塞、不改任何状态**。\n\
             什么时候用：用户问「这个会话到目前为止干了什么」「我们聊了多少轮」\
             「你调了几次工具」「撤销之后还剩什么」这一类关于**会话自身**的问题——\
             凭印象数是数不准的（历史会被压缩、被 undo 撤掉），这个工具读的是账本本身。\n\
             数字取的是**当前生效**的那一段：/undo 撤掉一轮之后再调，数字会跟着回落。",
        ),
        schema: Arc::new(json!({ "type": "object", "properties": {} })),
    }
}

/// timed 条目的声明。模型看不见它（timed 区不进 `specs()`），`description` 只给
/// 读代码的人和日志看。
pub fn audit_spec() -> ToolSpec {
    ToolSpec {
        name: Arc::from(AUDIT_TOOL),
        description: Arc::from("每个完成轮往 <session>.audit.log 追加一行审计记录。"),
        schema: Arc::new(json!({ "type": "object" })),
    }
}

/// 截获式执行体：渲染 + 把数字记进账。
///
/// 读写纪律（后代收窄、只走 command 面）在 [`crate::ext_stats_report`] 那边落实——
/// 这里连 `&mut Session` 都没用上，直接降成 `&Session` 交给纯函数。
fn report_run(ledger: Arc<Ledger>) -> SessionToolFn {
    Box::new(
        move |session: &mut Session, agent: &AgentId, _input: &Value| {
            let (body, counts) = ext_stats_report::render(session, agent);
            ledger.observe(counts);
            Ok(Arc::from(body))
        },
    )
}

/// `TurnEnd` 执行体：轮序号 +1，追加一行。
///
/// 失败回 `Err`——136 的驱动会记一条 `tracing::warn!` 然后接着跑下一条钩子
/// （结果丢弃、失败不影响这一轮）。不 `eprintln!`：CLI 的标准错误是给用户看对话的，
/// 一次写盘失败不该插进对话流里。
fn audit_run(ledger: Arc<Ledger>) -> TimedRun {
    Box::new(move |_table: &ToolTable, _input: &Value| ledger.append_turn_line())
}

/// 这个包自己的账：轮序号（钩子自己数）+ 最近一次 `report` 观测到的数字。
///
/// **住在宿主进程内存里，不进状态**——它不是 agent 的状态，undo 不该动它，恢复也不
/// 该从快照里长出来。跨重启的连续性靠一件事：轮序号[**从既有审计文件的行数续号**]
/// （[`Ledger::new`]），于是 `kill -9` 之后审计文件不会从 `turn=1` 重新数一遍。
pub struct Ledger {
    turns: AtomicU64,
    seen: Mutex<Option<Seen>>,
    audit: Option<PathBuf>,
}

/// 一次观测：哪一轮观测的、观测到什么。
#[derive(Clone, Copy)]
struct Seen {
    at_turn: u64,
    counts: Counts,
}

impl Ledger {
    /// 开一本账。轮序号从既有审计文件的**行数**续起——一行一轮是这个文件唯一的
    /// 结构承诺（验收 3），所以行数就是上一次跑到第几轮，不需要另存一份计数。
    pub fn new(audit: Option<PathBuf>) -> Arc<Self> {
        let seeded = audit.as_deref().map(existing_lines).unwrap_or(0);
        Arc::new(Ledger {
            turns: AtomicU64::new(seeded),
            seen: Mutex::new(None),
            audit,
        })
    }

    /// `report` 每次跑完记一笔。中毒（另一个线程持锁时 panic 了）也照用里面的值：
    /// 这格数据只喂审计行，为它把 CLI 拖垮不划算。
    ///
    /// 记的是**正在进行的那一轮**的序号（已完成轮数 + 1）：钩子在轮末才 +1，
    /// 拿它当时的值会把「第 2 轮里观测的」写成 `seen_at=turn1`——审计文件里差一位
    /// 的行号比没有行号更坑人（真机第一次跑出来就是这个样子，当场改的）。
    fn observe(&self, counts: Counts) {
        let at_turn = self.turns.load(Ordering::Relaxed) + 1;
        *self.seen() = Some(Seen { at_turn, counts });
    }

    fn seen(&self) -> MutexGuard<'_, Option<Seen>> {
        self.seen.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// 轮序号 +1 并追加一行。没有审计路径（临时会话）时只数轮，不写盘。
    fn append_turn_line(&self) -> Result<Arc<str>, Arc<str>> {
        let turn = self.turns.fetch_add(1, Ordering::Relaxed) + 1;
        let line = render_line(turn, *self.seen());
        let Some(path) = &self.audit else {
            return Ok(Arc::from("（临时会话，无审计文件）"));
        };
        append_line(path, &line)
            .map_err(|e| Arc::from(format!("审计行写不进 {}：{}", path.display(), e).as_str()))?;
        Ok(Arc::from(line.as_str()))
    }

    /// 测试用：此刻数到第几轮。
    pub fn turns(&self) -> u64 {
        self.turns.load(Ordering::Relaxed)
    }
}

/// 一行审计长什么样。**固定字段、固定顺序、没有时钟**——这个文件是拿来 `wc -l` 和
/// `grep` 的，一行一轮；从没观测过就老实写 `-`，不拿零顶替（「这一轮 0 条 entry」
/// 和「这一轮没人观测过」是两件事）。
fn render_line(turn: u64, seen: Option<Seen>) -> String {
    match seen {
        None => format!("turn={turn} entries=- turns=- agents=- tools=- seen_at=-"),
        Some(Seen { at_turn, counts }) => format!(
            "turn={turn} entries={}/{} turns={} agents={} tools={} seen_at=turn{at_turn}",
            counts.effective, counts.entries, counts.turns, counts.agents, counts.tool_calls,
        ),
    }
}

/// 追加一行（含换行）。`create(true).append(true)`：文件不在就建，多个进程先后写
/// 同一份也只是接着往后追。
fn append_line(path: &Path, line: &str) -> std::io::Result<()> {
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    file.write_all(line.as_bytes())?;
    file.write_all(b"\n")
}

/// 既有审计文件有几行。读不到（不存在 / 没权限）就当 0——续号是锦上添花，不能因为
/// 它让一个正常的会话起不来。
fn existing_lines(path: &Path) -> u64 {
    std::fs::read(path)
        .map(|bytes| bytes.iter().filter(|b| **b == b'\n').count() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
#[path = "ext_stats_tests.rs"]
mod tests;
