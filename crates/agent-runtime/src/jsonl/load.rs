//! `load()`：从磁盘直接重放，不经 IO 线程的活体镜像——一份全新的 `Jsonl` 实例
//! （对应「进程重启」）根本没有活体镜像，`load` 必须只靠文件内容就能重建正确状态，
//! 这个函数就是唯一入口，验证的也正是这条路径。
//!
//! 崩溃语义（issue 011 硬约束，返回类型三态化见 `agent_store::persist::LoadOutcome`
//! 文档「契约更正」一节）：
//! - **尾部半行**（最后一行解析失败）：容忍，从该行截断，经 `on_error` 报 warn，
//!   前面的内容照常加载——append-only 写到一半断电的诚实语义，结果是 `Loaded`。
//! - **中部损坏**（非最后一行解析失败）：整份拒绝，经 `on_error` 报错（哪一行），
//!   不静默丢中段、不加载半份状态——结果是 `Refused`，**不是** `Absent`：这个身份
//!   下明明写过东西，只是这一份数据现在读不出来，宿主不能把它当「从没写过」处理
//!   （那正是三态化之前的真 bug：`main.rs` 拿 `None` 开新会话，下一张快照就把
//!   损坏之前还能人工修复的原文件覆盖了）。
//! - **文件不存在/读到但从没写过任何记录**：`Absent`，开新会话是对的。
//!
//! 三种情况都不 panic。`on_error`/`Refused.reason` 里只带行号一类的分类信息，
//! 不带那一行的内容——里面可能是 `Entry`/`Snapshot` 序列化出来的用户对话
//! （红线：绝不打印 K/V 内容）。
//!
//! ## `seed_from_disk`：IO 线程启动时也要读一遍这份文件（真 bug 1 的另一半）
//!
//! `io_thread.rs` 里那份连续存活的 `SessionLog`（`mirror`）以前恒等于
//! `SessionLog::new()`——「进程重启」意味着一个全新 `Jsonl` 实例、一条全新 IO
//! 线程，如果文件本来就已经有内容（上一个进程写的，没有被任何快照压实过），
//! 这份新镜像对那些内容一无所知：`boundary` 停在 0、`held` 是空的，但物理文件
//! 里明明已经有 N 条 entries。
//!
//! 独测的「三个连续重启周期」回归测试抓到了这条的后果：第二个周期结束时，
//! `mirror.held` 只有**这个进程自己**这一轮新写的那几条，`SetCursor` 落盘的
//! `relative_cursor()`（`cursor.min(held.len())`）因此被系统性地算小了——用的
//! 是「这个进程见过多少条」而不是「文件里实际有多少条」。第三个周期 recover 出
//! 一个 `cursor < entries.len()` 的会话（明明什么都没 undo 过），它自己的下一次
//! 写入被 `History` 当成「覆盖 redo 尾」处理，第二个周期真实写过的整轮对话被一条
//! `drop_after` 悄悄冲掉——不 panic、不报错，正是 `docs/INVARIANTS.md` 点名的
//! 那类最贵的静默错值 bug，只是这次出现在 cursor 通道而不是 seq 通道
//! （`RunnerCtx::persisted_seq` 那半个修法管的是 seq 通道，管不到这里）。
//!
//! 修法：`io_thread::run` 起步时用这个函数把 `mirror` 追平到文件已有内容——跟
//! `load()` 走同一条重放逻辑，唯一差别是 `on_error` 换成静默：真正的错误报告
//! 属于应用层显式调用 `load()`/`recover()` 的那一次（`main.rs` 的路径就是这样），
//! 这里只是让 IO 线程自己的内部记账不再对着一份不完整的世界观做算术；文件不存在
//! /读不出来/中部损坏都退化成一份空日志——那些情况下 `recover()` 会各自给出
//! `Absent`/`Refused`，宿主要么开新会话要么直接硬失败退出，压根不会走到需要这份
//! 镜像的下一次写入。

use std::fs;
use std::path::Path;

use serde::de::DeserializeOwned;

use agent_store::persist::{LoadOutcome, SessionLog};

use super::error::SessionStoreError;
use super::record::Record;

/// [`replay`] 的产物：区分「文件不存在」「读/解析被拒绝」「重放出一份日志」——
/// 跟 [`LoadOutcome`] 同一个三态划分，只是这里还没决定日志是不是"从没写过东西"
/// （那要等 [`SessionLog::to_loaded`] 判断），[`load`] 和 [`seed_from_disk`] 各自
/// 把这个中间态收敛成自己需要的形状。
enum Replayed<K, V, M> {
    NotFound,
    Refused(String),
    Ok(SessionLog<K, V, M>),
}

pub(super) fn load<K, V, M>(
    path: &Path,
    on_error: &(dyn Fn(SessionStoreError) + Send + Sync),
) -> LoadOutcome<K, V, M>
where
    K: Clone + DeserializeOwned,
    V: Clone + DeserializeOwned,
    M: Clone + DeserializeOwned,
{
    match replay(path, on_error) {
        Replayed::NotFound => LoadOutcome::Absent,
        Replayed::Refused(reason) => LoadOutcome::Refused { reason },
        Replayed::Ok(log) => log.to_loaded().map_or(LoadOutcome::Absent, LoadOutcome::Loaded),
    }
}

/// 见模块文档「`seed_from_disk`」一节：`io_thread::run` 起步时调一次，静默—— 不
/// 报 `on_error`（那是应用层显式 `load()` 的职责），任何读不出来的情况都退化成
/// 一份空日志，不阻塞 IO 线程启动。
pub(super) fn seed_from_disk<K, V, M>(path: &Path) -> SessionLog<K, V, M>
where
    K: Clone + DeserializeOwned,
    V: Clone + DeserializeOwned,
    M: Clone + DeserializeOwned,
{
    match replay(path, &|_: SessionStoreError| {}) {
        Replayed::Ok(log) => log,
        Replayed::NotFound | Replayed::Refused(_) => SessionLog::new(),
    }
}

fn replay<K, V, M>(path: &Path, on_error: &(dyn Fn(SessionStoreError) + Send + Sync)) -> Replayed<K, V, M>
where
    K: Clone + DeserializeOwned,
    V: Clone + DeserializeOwned,
    M: Clone + DeserializeOwned,
{
    let content = match fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Replayed::NotFound, // 全新会话
        Err(e) => {
            // 读不出来（权限/其它 IO 错误），但不是「文件不存在」——不知道底下
            // 有没有真数据，按「有会话但拒绝加载」处理才安全：`Absent` 会诱使
            // 宿主开一个新会话，下一次写入就可能把打不开的原文件盖掉。
            let err = SessionStoreError::Io { detail: e.kind().to_string() };
            let reason = err.to_string();
            on_error(err);
            return Replayed::Refused(reason);
        }
    };

    let lines: Vec<&str> = content.lines().collect();
    let mut log: SessionLog<K, V, M> = SessionLog::new();

    for (i, line) in lines.iter().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let is_last = i + 1 == lines.len();
        match serde_json::from_str::<Record<K, V, M>>(line) {
            Ok(record) => apply(&mut log, record),
            Err(_) if is_last => {
                // 尾部半行：容忍，从这里截断——后面本来就没有更多行了。
                on_error(SessionStoreError::TruncatedTail { line: i + 1 });
                break;
            }
            Err(_) => {
                // 中部损坏：整份拒绝，不加载半份状态——`Refused`，不是 `Absent`。
                let err = SessionStoreError::CorruptLine { line: i + 1 };
                let reason = err.to_string();
                on_error(err);
                return Replayed::Refused(reason);
            }
        }
    }

    Replayed::Ok(log)
}

fn apply<K: Clone, V: Clone, M: Clone>(log: &mut SessionLog<K, V, M>, record: Record<K, V, M>) {
    match record {
        Record::Entry(e) => log.record_append(&e),
        Record::Snapshot(s) => log.record_snapshot(&s),
        Record::Cursor { cursor } => log.record_cursor(cursor),
        Record::DropOldest { count } => {
            log.record_drop_oldest(count);
        }
        Record::DropAfter { first_seq, count } => log.record_drop_after(first_seq, count),
    }
}
