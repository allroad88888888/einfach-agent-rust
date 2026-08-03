//! 原子图的**地址空间**：[`AtomKey`]（落盘的逻辑键，红线 4）、它的两级槽位枚举，
//! 以及每个槽位的默认值。
//!
//! 这个文件只回答两个问题：**「一个槽位怎么称呼」**和**「它没有值的时候是什么」**。
//! 「谁来建它」在同目录的 [`build`](super::build)，「谁来写它」在 `command/`。
//!
//! ## 为什么键是逻辑键
//!
//! `AtomId` 是自增 `u64`，完全依赖创建顺序：快照存 `(AtomId, Value)` 的话，只要有人
//! 往构图函数中间插一行 `create_atom`，所有旧快照的值就整体错位——**而且不报错**
//! （红线 4）。`AtomKey` 是「怎么还原」（`Slot`）+「还原哪一个」（`AgentId`），
//! 与创建顺序无关，于是快照能跨进程、日志能跨版本、019 的按需重建才有依据
//! （拿不到 `Slot` 就不知道该建什么）。
//!
//! 顺带白拿 schema 演进：新增槽位在旧快照里找不到键，用 [`Slot::default_value`]；
//! 删掉的槽位在快照里是多余项，忽略即可。不需要迁移脚本。
//!
//! ## `Slot` 还是个子集，`AtomKey` 不是
//!
//! `Slot` 照 `docs/STATE-MODEL.md` 的槽位表**裁剪**到真的有写入点的那些：
//! `config` / `system_base` / `skills_active` / `tools_registry_version` 现在没有任何
//! 写入点，先不定（021 的教训：没被真实使用验证过的槽位，跟没写一样，只是它看起来
//! 像做完了）。028 只加了一个 [`Slot::ToolsAllowed`]——它有写入点
//! （`Session::spawn_child`）也有读者（029 的子 agent 工具表 + 活名单判定）。
//!
//! 每个槽位还要回答第三个问题「别的 agent 能不能读它」，那是隔壁
//! [`visibility`](super::visibility) 的事（红线 10）。
//!
//! `AtomKey` 的**两个变体一个不少**，即使 M2 只构造 `Agent` 那一支：它是落盘键的
//! 类型，改它的形状等于让所有旧日志/快照解不出来。`Slot` 可以往里加（旧快照缺键
//! 用默认值），`AtomKey` 的变体集合不能事后改——两者的稳定性要求不是一个量级。

use serde::{Deserialize, Serialize};

use crate::engine::state::{DEFAULT_MAX_RETRIES, DEFAULT_MAX_TURNS, TurnStatus};
use crate::ids::{AgentId, ToolCallId};
use crate::value::atom_value::AgentValue;

/// 一个 agent 的槽位。**只有 source（primitive）槽位**——derived 不进日志、不进
/// 快照，它们的键是 [`DerivedKey`]，两套键分开正是为了让「快照只存 primitive」
/// 成为类型上的事实而不是纪律。
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Serialize, Deserialize)]
pub enum Slot {
    /// 消息历史。
    Messages,
    /// 这一轮走到哪了。
    Status,
    /// 本轮的工具槽，顺序 = 模型请求顺序。
    ToolSlots,
    /// 上一次请求的前缀镜像；第一轮之前是 [`AgentValue::Null`]。
    PrevPrefix,
    /// 下一个要铸的 `MessageId`（从 1 起严格递增）。
    NextMessageId,
    /// 本轮已经发起的 `CallProvider` 次数（新一轮和重试都算）。
    TurnsUsed,
    MaxTurns,
    /// 当前这条失败-重试链已经连续失败了几次。
    RetriesUsed,
    MaxRetries,
    /// **spawn 当时快照的工具子集**（028 唯一新增的槽位，029 消费）。
    ///
    /// 值是 [`AgentValue::Json`] 里的一个字符串数组，`Null` = 「这个 agent 不在
    /// 活名单上」。两件事共用一个槽位不是省事，是它们本来就是一件事：
    /// **「这个 agent 是被 spawn 出来的，带着这份工具子集」**——`Null` 是这个事实
    /// 的缺席，不是第二个字段。于是「从没 spawn 过」「spawn 被 undo 掉了」
    /// 「已经 despawn」三种情况在状态上完全一致，因为它们**就是**同一种状态。
    ///
    /// 为什么是 spawn 当时的快照而不是现查工具表：和 `ToolCallSlot::Request` 存
    /// 发起时 `Reversibility` 是同一个道理（issue 006 §注意）——undo 回到 spawn
    /// 那一刻，用的必须是当时的工具表，不是现在的。
    ///
    /// 排序去重后落盘（红线 11）：它会被渲染进子 agent 的 prompt，顺序一漂前缀
    /// 缓存就全价。写入点在 `Session::spawn_child`。
    ToolsAllowed,
    /// **当前激活的 skill id 列表**（039）。值是 [`AgentValue::Json`] 里一个
    /// **排序去重的字符串数组**，`Json([])` = 没有激活任何 skill（默认值）。
    ///
    /// store 里只存「哪些被激活」，skill 的正文/工具在 store 外的 registry 里
    /// （TOOLS.md §Skills；也是 `AtomKey` 没有 `Skill` 变体的原因）——于是
    /// undo 撤一次激活就退化成一次普通的值回滚（跟 `ToolsAllowed` 一视同仁），
    /// 崩溃恢复靠这个 primitive 自动回来、正文从 registry 现取。
    ///
    /// 为什么是排序去重的数组而不是 `HashSet`（红线 11）：它会被 registry 展开成
    /// 注入进 system prompt 的正文，顺序一漂前缀缓存就全价。写入点在
    /// `Session::activate_skill` / `deactivate_skill`，那两处落值前排序去重。
    SkillsActive,
}

/// 一次工具调用自己的槽位。
///
/// M2 只有 `Result` 一个：`Request`（发起当时的 `Location` / `Reversibility` 快照，
/// STATE-MODEL §「落盘的键必须是 AtomKey」）要等**持有工具表的宿主**来记——core
/// 没有工具表，现造一份占位快照是编造（002 合并时的裁决：假的 `Irreversible`
/// 会让 undo 白拦一次 `fs/read`，正是静默错值）。
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Serialize, Deserialize)]
pub enum ToolCallSlot {
    /// 在飞时持 [`AgentValue::Pending`]，回来后持内容。
    Result,
}

/// 落盘的逻辑键。`Snapshot` 与 `Entry.changes` 用它，`AtomId` 只在进程内有效。
///
/// **只有两个变体**。没有 `Skill(SkillId)`——skill 的内容在 store 外的 registry 里，
/// store 里只有「哪些被激活」，那是某个 `Agent(_, _)` 槽位（STATE-MODEL）。
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Serialize, Deserialize)]
pub enum AtomKey {
    Agent(AgentId, Slot),
    ToolCall(AgentId, ToolCallId, ToolCallSlot),
}

impl AtomKey {
    /// 这个槽位「没有值」的时候是什么。
    ///
    /// **唯一的一处**：构图函数建 atom 用它，019 的按需重建走的是同一个构图函数、
    /// 因此也是同一份默认值。分成两份的那一刻，undo 路径重建出来的 atom 就会和
    /// 正常创建出来的不一样——而那条路径只有「长会话 + 逐出 + undo」三件事同时
    /// 发生才走得到，通常是在线上。
    pub fn default_value(&self) -> AgentValue {
        match self {
            AtomKey::Agent(_, slot) => slot.default_value(),
            AtomKey::ToolCall(_, _, ToolCallSlot::Result) => AgentValue::Pending,
        }
    }

    /// 这个键属于哪个 agent。`undo` 不看它（一条扁平日志按时间排序），
    /// 逐出与 UI 时间线看它。
    pub fn agent(&self) -> &AgentId {
        match self {
            AtomKey::Agent(a, _) | AtomKey::ToolCall(a, _, _) => a,
        }
    }
}

impl Slot {
    /// 见 [`AtomKey::default_value`]。上限两项取的是 `engine::state` 的同一对常量
    /// ——M1 引擎与 `Session` 的默认预算必须是同一个数，否则「行为一条不许变」
    /// 就退化成一句要靠人核对的话。
    pub fn default_value(self) -> AgentValue {
        match self {
            Slot::Messages => AgentValue::Messages(imbl::Vector::new()),
            Slot::Status => AgentValue::Status(TurnStatus::Idle),
            Slot::ToolSlots => AgentValue::Slots(std::sync::Arc::new(Vec::new())),
            Slot::PrevPrefix => AgentValue::Null,
            Slot::NextMessageId => AgentValue::U64(1),
            Slot::TurnsUsed => AgentValue::U64(0),
            Slot::MaxTurns => AgentValue::U64(DEFAULT_MAX_TURNS as u64),
            Slot::RetriesUsed => AgentValue::U64(0),
            Slot::MaxRetries => AgentValue::U64(DEFAULT_MAX_RETRIES as u64),
            // `Null` = 不在活名单上。**默认值必须是「不活着」**：019 的按需重建
            // 拿的就是这个默认值，若默认成「活着」，undo 路径上凭空重建出来的
            // atom 会让一个早就 despawn 的子 agent 复活——链通、值错、不报错。
            Slot::ToolsAllowed => AgentValue::Null,
            // 「没有激活任何 skill」= 一个**空的有序数组**，不是 `Null`：SkillsActive
            // 永远持一个数组（跟 ToolSlots 永远持 Slots 同一个道理），读取点因此不必
            // 区分「空」和「类型错」。空数组序列化成 `[]`，逐字节确定（红线 11）。
            Slot::SkillsActive => AgentValue::Json(std::sync::Arc::new(serde_json::Value::Array(
                Vec::new(),
            ))),
        }
    }

    /// 一个 agent 的全部 source 槽位。`Session::new` 建图、`Session::primitives`
    /// 出快照都用它——**新增槽位只要加进这个数组，两条路径自动跟上**，
    /// 忘了改其中一条正是「快照缺一块」的来源。
    ///
    /// 新槽位**追加在末尾**：旧快照里找不到新键，按 [`Slot::default_value`] 落值
    /// （schema 演进白拿的那一条），而追加不改动既有槽位的相对次序，
    /// 快照的排序输出因此在版本之间是稳定的。
    pub const ALL: [Slot; 11] = [
        Slot::Messages,
        Slot::Status,
        Slot::ToolSlots,
        Slot::PrevPrefix,
        Slot::NextMessageId,
        Slot::TurnsUsed,
        Slot::MaxTurns,
        Slot::RetriesUsed,
        Slot::MaxRetries,
        Slot::ToolsAllowed,
        Slot::SkillsActive,
    ];
}

/// derived atom 的键。**刻意不 derive serde**：derived 不进日志也不进快照
/// （它们全部可重算，这正是「完整状态 = 所有 primitive」成立的原因），给它一个
/// `Serialize` 就是给「把算出来的值也存一份」开了口子。
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum DerivedKey {
    /// 「本 agent 的工具槽全都不是 `Pending` 了吗」。003 预言的那个 derived。
    ToolsConverged(AgentId),
}
