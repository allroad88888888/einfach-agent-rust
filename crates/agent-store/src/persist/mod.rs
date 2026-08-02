//! `SessionStore` 端口（issue 011）：把 [`History`](crate::History) 的写入镜像到进程外，
//! 好在 `kill -9` 之后把会话接回来。`docs/STATE-MODEL.md` §「持久化」定了两条刻意的设计：
//!
//! **写入全部 fire-and-forget，没有返回值。** 失败不回滚内存状态，只经构造时注入的
//! `on_error` 上报——一次 IO 抖动不能让 undo 永久卡死（上游 TS 版的教训）。
//!
//! **同步 trait。** actor 是单线程的，真实现自己把写扔给专门的 IO 线程；`agent-core`
//! 不用染上 async。
//!
//! ## 与原 issue 文本的一处修正
//!
//! 原 issue 草案里每个方法都带一个 `SessionId` 参数。实做时改成**一个实例绑一个会话**
//! （构造时给身份/路径）：`docs/STATE-MODEL.md` §「子agent」已经钉死「一个 root agent +
//! 整棵子树 = 一个 session = 一个 actor 线程 = 一个 store」，M3 的多会话是「每会话一个
//! `SessionStore` 实例」，不是「一个实例带 id 路由」——路由到哪个文件/哪张表是宿主
//! （agent-runtime 的会话管理器）的事，不该是这个端口的事。
//!
//! ## 两个实现住在哪
//!
//! `Memory`（本模块）与端口定义一样零 IO（红线 7）：`Vec`/字段直存，测试与临时会话用。
//! `Jsonl`（真正做文件 IO 的实现）住在 `agent-runtime`——`agent-core` 和 `agent-store`
//! 都被红线 7 禁着，唯一能做 IO 的地方是运行时层。
//!
//! `Jsonl` 与 `Memory` 共用同一套「游标怎么翻译、snapshot 怎么压实」的逻辑
//! （[`SessionLog`](log::SessionLog)，见该模块文档）——两个实现分叉一次这套算法，
//! 验收要求的「写→load→重放语义一致」就成了两份各自维护的推导，迟早对不上。

mod log;
mod memory;

pub use log::SessionLog;
pub use memory::Memory;

use crate::history::{Entry, Snapshot};

/// 会话持久化端口。
pub trait SessionStore<K, V, M> {
    /// 追加一条 entry。**fire-and-forget**：这次写要是失败了，只经 `on_error` 上报，
    /// 内存里的 `History` 该怎样还怎样——undo/redo 不等这次写成功。
    fn append(&self, entry: &Entry<K, V, M>);

    /// cap 溢出的转发落点（[`DropEvent::Oldest`](crate::history::DropEvent::Oldest)）：
    /// 从最老一端丢了 `count` 条。
    fn drop_oldest(&self, count: usize);

    /// 分支覆盖的转发落点（[`DropEvent::RedoTail`](crate::history::DropEvent::RedoTail)）：
    /// 从 `first_seq` 开始的 `count` 条 redo 尾被新写入覆盖丢弃。
    fn drop_after(&self, first_seq: u64, count: usize);

    /// 游标挪到了哪。传的是 [`History::cursor`](crate::History::cursor) 的原样值——
    /// 相对 `History` 自己**当前**那份 `entries`（已经把 cap 驱逐的缩短算在内了，见
    /// `history/cap.rs::enforce_cap`），不是「相对最近一次快照」的值。调用方不需要为
    /// 这个端口另做换算，换算是实现自己的事（[`SessionLog`] 的职责）。
    fn set_cursor(&self, cursor: usize);

    /// 落一张快照：**之后的 `load` 以它为基线**，它之前的 entries 允许被实现压实
    /// （重写文件、清空内存里的旧副本，怎么做由实现决定）。
    fn snapshot(&self, snap: &Snapshot<K, V>);

    /// 载入。三态见 [`LoadOutcome`]——`Option` 曾经把「文件不存在」和「有会话但
    /// 拒绝加载（中部损坏/不变量破坏）」都压缩成 `None`，宿主没法区分，"没有会话"
    /// 与"有会话但读不出来"对宿主是两件完全不同的事：前者开新会话是对的，后者
    /// 必须硬失败——继续跑，下一张快照就把现场覆盖了（见 [`LoadOutcome`] 文档）。
    fn load(&self) -> LoadOutcome<K, V, M>;
}

/// [`SessionStore::load`] 的产物：三态，不是 `Option`。
///
/// ## 契约更正（027 独测发现，2026-08-02）：从 `Option<LoadedSession>` 三态化
///
/// 011/027 原本的签名是 `fn load(&self) -> Option<LoadedSession<K, V, M>>`——`None`
/// 身兼两职：「这个身份下从来没写过东西」（开新会话是对的）与「`Jsonl::load()` 自己
/// 因为中部损坏拒绝加载」（`agent-runtime/src/jsonl/load.rs` 的 `CorruptLine` 分支，
/// 011 的崩溃语义早就设计了这条，只是把结果压回了同一个 `None`）。宿主（`agent-cli
/// ::main`）拿到 `None` 只有一条路可走：当成「全新会话」——这正是独测抓到的真
/// bug：中部损坏的会话文件被误判成「没有会话」，警告打了，但接下来第一张快照
/// （`SessionStore::snapshot` 的截断语义）就把用户原文件覆盖了，损坏之前还能靠人工
/// 修复的数据这下真的没了。
///
/// 修法：`load()` 返回这个三态 `enum`，`Absent` 与 `Refused` 现在是两个不同的
/// 值，调用方（`agent_runtime::persist::recover`）把 `Refused` 翻成
/// `RecoverError`，`main.rs` 走它已有的硬失败出口——不新增分支，是堵上一个
/// 曾经悄悄坍缩成同一个值的坑。`Memory`（本模块）没有序列化步骤，天生不会
/// `Refused`，只在 `Jsonl`（agent-runtime）一侧真的出现。
pub enum LoadOutcome<K, V, M> {
    /// 这个身份下从来没写过东西——文件不存在，或者写过但内容为空/从未真正落过一条
    /// 记录。开新会话是对的。
    Absent,
    /// 有会话，但这一份数据不能安全加载（中部损坏、结构不满足实现自身的完整性
    /// 要求）。`reason` 只带类别与行号一类的诊断信息，**不带 K/V 内容**——那里面
    /// 可能是用户对话（跟 `agent-runtime::SessionStoreError` 同一条红线）。宿主
    /// 必须硬失败，让人先备份现场：静默降级成新会话，下一次持久化写入就会把
    /// 旧数据压掉。
    Refused { reason: String },
    /// 正常载入。
    Loaded(LoadedSession<K, V, M>),
}

impl<K, V, M> LoadOutcome<K, V, M> {
    /// `true` 当且仅当这个身份从来没写过东西。
    pub fn is_absent(&self) -> bool {
        matches!(self, LoadOutcome::Absent)
    }

    /// `true` 当且仅当有会话但被拒绝加载。
    pub fn is_refused(&self) -> bool {
        matches!(self, LoadOutcome::Refused { .. })
    }

    /// 只有 `Loaded` 才给 `Some`——`Absent`/`Refused` 都不是「有一份可用数据」。
    /// **生产代码判断「要不要开新会话」不能走这个方法**：那正好会把 `Refused`
    /// 悄悄坍缩回 `None` 的旧行为，三态化就白做了。这个方法是给已经用
    /// `match`/`is_absent`/`is_refused` 显式分过支、只是想拿内层值的调用方
    /// （多是测试）用的。
    pub fn loaded(self) -> Option<LoadedSession<K, V, M>> {
        match self {
            LoadOutcome::Loaded(loaded) => Some(loaded),
            LoadOutcome::Absent | LoadOutcome::Refused { .. } => None,
        }
    }
}

/// [`SessionStore::load`] 的产物，`Loaded` 那一态里装的东西。
pub struct LoadedSession<K, V, M> {
    pub snapshot: Option<Snapshot<K, V>>,
    /// 快照点之后的日志，按追加顺序。
    pub entries: Vec<Entry<K, V, M>>,
    /// **相对这份 `entries`的游标**（`0..=entries.len()`），可以直接喂给
    /// `History::from_parts(entries, cursor, next_seq)`——那个函数会校验
    /// `cursor <= entries.len()`，实现有责任在这里就满足它，不能把校验的麻烦甩给调用方。
    pub cursor: usize,
    /// 下一个要铸的 seq。即使 `entries` 因为压实是空的，这个数也不会跌回 0
    /// ——seq 不回收（`docs/STATE-MODEL.md`）。
    pub next_seq: u64,
}
