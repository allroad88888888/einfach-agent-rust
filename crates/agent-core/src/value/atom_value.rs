//! 原子图上流动的唯一值类型：[`AgentValue`]。
//!
//! **这是 command log 与快照的值 schema**（红线 3：primitive 的值必须全部可序列化）。
//! 一条 `Entry` 落盘之后，它里面的 `prev`/`next` 就是这个枚举的序列化形式——所以
//! 变体集合是**封闭的**：事后加一个变体等于一次 schema 迁移，事后改一个变体的载荷
//! 等于让旧日志静默解错。026 一次定死，`Slot` 表可以随里程碑长，值的形状不再动。
//!
//! ## 为什么恰好是这十个
//!
//! | 变体 | 谁在用 |
//! |------|--------|
//! | `Null` | 「没有值」：`PrevPrefix` 在第一轮之前；也是 [`AtomValue::null`]，019 重建缺席 atom 时的默认值 |
//! | `Pending` | 「还在等」：`ToolCall(_, _, Result)` 在飞时持它；`tools_converged` 未收敛时也答它（`Pending` 沿依赖图汇聚，STATE-MODEL §「Pending 的来历」） |
//! | `Bool` | `tools_converged` 收敛时的答案 |
//! | `U64` | 六个计数槽位：epoch 之外的 `NextMessageId` / `TurnsUsed` / `MaxTurns` / `RetriesUsed` / `MaxRetries`，以及 M3 的 `tools_registry_version` |
//! | `Text` | 一段不可变文本：M3 的 `SystemBase`、`ToolCall(_, _, Result)` 落地后的内容 |
//! | `Json` | 未解析的工具入参（core 不解析参数含义，只透传）；也装「有序字符串集」——`ToolsAllowed` 的工具子集、`SkillsActive` 的激活 skill id（`value::str_set`，红线 11 排序去重） |
//! | `Messages` | 消息历史 |
//! | `Status` | `TurnStatus` |
//! | `Prefix` | 上一次请求的前缀镜像 |
//! | `Slots` | 本轮的工具槽，顺序 = 模型请求顺序 |
//!
//! `Text` / `Json` / `Pending` 的**主**写入点在 `AtomKey::ToolCall` 那一族槽位上，
//! 那是 M3 的子 agent / 逐出路径要用的形状；M2 单 agent 的工具槽整体住在
//! `Slots` 一个槽位里（003 预言的「扫槽位形状原样搬过去」）。先定形状是因为
//! 上面那条：值 schema 一旦有日志落过盘就动不得了。
//!
//! ## 明确**不**提供的
//!
//! `Opaque(Arc<dyn Any>)` 这类逃生口一个都没有（红线 3）。在飞的 HTTP stream、
//! MCP 子进程、`JoinHandle` 不是状态，是状态的执行现场——它们住在 store 外面的
//! runtime registry，atom 里只放一个可序列化的句柄。给了这种变体就一定有人塞，
//! 然后快照有洞，而且是等到第一次真的从崩溃恢复时才发现的洞。

use std::sync::Arc;

use imbl::Vector;
use serde::{Deserialize, Serialize};

use crate::engine::state::{ToolSlot, TurnStatus};
use crate::seam::PrefixImage;
use crate::value::message::Message;

/// 原子图上的一个值。
///
/// # `PartialEq` 是手写的（红线 5）
///
/// `store.set` 靠 `PartialEq` 判断「变没变」来决定要不要传播，`record_set` 靠同一个
/// 判断决定要不要落一条 `Change`——一千条消息的会话里，每次写入都深比较一遍历史是
/// 性能悬崖。所以每个可能变大的变体第一分支都是指针相等：`Arc::ptr_eq`，
/// `Messages` 用 `imbl::Vector::ptr_eq`（结构共享的指针快路）。
///
/// 指针不等时**仍然继续深比较**，不是直接判不等：`ptr_eq` 是快路不是语义。
/// 判不等的代价是一次多余的传播 + 一条内容相同的 `Change`（undo 时按一下没反应的
/// 幽灵步），比多花几微秒贵。
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum AgentValue {
    /// 没有值。`PrevPrefix` 在第一轮之前是它，被 019 重建出来的 atom 也先落它。
    Null,
    /// 还在等。在飞的工具槽持它，未收敛的汇聚型 derived 也答它。
    Pending,
    Bool(bool),
    U64(u64),
    /// 一段不可变文本。`Arc<str>` 而不是 `String`：克隆是指针拷贝（红线 5）。
    Text(Arc<str>),
    /// 未解析的 JSON（工具入参）。core 不解析参数含义，schema 是工具自己的事。
    Json(Arc<serde_json::Value>),
    /// 消息历史。`imbl::Vector` 而不是 `Arc<Vec>`：append 是 O(log n) 且结构共享，
    /// undo 日志里存旧版本几乎零成本（STATE-MODEL §「消息历史用持久化向量」）。
    Messages(Vector<Message>),
    Status(TurnStatus),
    Prefix(PrefixImage),
    /// 本轮的工具槽，**顺序就是模型请求的顺序**。`Vec` 不是 map：结果要按这个顺序
    /// 喂回去（红线 11 的精神），而 `HashMap` 的迭代顺序是随机的。
    Slots(Arc<Vec<ToolSlot>>),
}

impl PartialEq for AgentValue {
    fn eq(&self, other: &Self) -> bool {
        use AgentValue::*;
        match (self, other) {
            (Null, Null) | (Pending, Pending) => true,
            (Bool(a), Bool(b)) => a == b,
            (U64(a), U64(b)) => a == b,
            // 三条 `Arc` 快路：同一个分配就不必看内容。
            (Text(a), Text(b)) => Arc::ptr_eq(a, b) || a == b,
            (Json(a), Json(b)) => Arc::ptr_eq(a, b) || a == b,
            (Slots(a), Slots(b)) => Arc::ptr_eq(a, b) || a == b,
            // `imbl` 的结构共享快路：push_back 之后新旧两份共享绝大部分节点，
            // 但根不同 → `ptr_eq` 为假 → 落到深比较，这正是我们要的答案（真的变了）。
            (Messages(a), Messages(b)) => a.ptr_eq(b) || a == b,
            (Status(a), Status(b)) => a == b,
            (Prefix(a), Prefix(b)) => a == b,
            _ => false,
        }
    }
}

impl agent_store::AtomValue for AgentValue {
    /// 019 重建缺席 atom 时的兜底值，也是 store 超出递归预算时的占位值。
    ///
    /// **注意它不是「这个槽位的默认值」**——那是 `Slot::default_value()`
    /// （`graph/slot.rs`）的事，按逻辑键决定。applier 一律经调用方的 `resolve`
    /// 拿 atom，所以正常路径上永远看不到这个 `Null`。
    fn null() -> Self {
        AgentValue::Null
    }
}

/// 取值：类型对不上一律返回 `None`，由调用方决定是 `expect` 还是兜底。
///
/// command 层的读取点全部 `expect`——槽位与变体的对应关系由构图函数一处焊死
/// （`Slot::default_value()`），对不上说明构图函数和读取点已经不同步了，
/// 那是必须当场炸掉的 bug，不是可以静默兜底的运行时情况（本仓最恨静默错值）。
impl AgentValue {
    pub fn as_bool(&self) -> Option<bool> {
        match self {
            AgentValue::Bool(b) => Some(*b),
            _ => None,
        }
    }

    pub fn as_u64(&self) -> Option<u64> {
        match self {
            AgentValue::U64(n) => Some(*n),
            _ => None,
        }
    }

    pub fn as_text(&self) -> Option<&Arc<str>> {
        match self {
            AgentValue::Text(t) => Some(t),
            _ => None,
        }
    }

    pub fn as_json(&self) -> Option<&Arc<serde_json::Value>> {
        match self {
            AgentValue::Json(j) => Some(j),
            _ => None,
        }
    }

    pub fn as_messages(&self) -> Option<&Vector<Message>> {
        match self {
            AgentValue::Messages(m) => Some(m),
            _ => None,
        }
    }

    pub fn as_status(&self) -> Option<&TurnStatus> {
        match self {
            AgentValue::Status(s) => Some(s),
            _ => None,
        }
    }

    /// `Null` 是「没有前缀镜像」，不是错误——所以这里返回 `Option<Option<_>>` 会
    /// 很难用，改成：`Null` → `None`，`Prefix(p)` → `Some(p)`，其余变体同样 `None`。
    /// 读取点因此不需要区分「槽位是空的」和「槽位类型错了」，因为构图函数保证了
    /// 这个槽位只可能是这两个变体之一。
    pub fn as_prefix(&self) -> Option<&PrefixImage> {
        match self {
            AgentValue::Prefix(p) => Some(p),
            _ => None,
        }
    }

    pub fn as_slots(&self) -> Option<&Arc<Vec<ToolSlot>>> {
        match self {
            AgentValue::Slots(s) => Some(s),
            _ => None,
        }
    }

    /// 「还在等」。汇聚型 derived 用它短路（读到第一个 `Pending` 就能返回，
    /// 不用读完——STATE-MODEL §「汇聚 atom 的复杂度」）。
    pub fn is_pending(&self) -> bool {
        matches!(self, AgentValue::Pending)
    }
}
