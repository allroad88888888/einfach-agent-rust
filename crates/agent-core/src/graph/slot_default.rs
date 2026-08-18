//! 「一个槽位没有值的时候是什么」——[`Slot::default_value`] 与
//! [`AtomKey::default_value`]，**唯一的一处**。
//!
//! 从 [`slot`](super::slot) 拆出来（107 加 `Slot::Summaries` 时顶破 300 行）：那个
//! 文件原本的模块文档自己就写着「只回答两个问题：一个槽位怎么称呼、它没有值的
//! 时候是什么」——两个问题就是两件事。名字（枚举、逻辑键、`ALL` 表）留在那边，
//! 缺席值搬到这里，两边各自能一句话说清自己是干嘛的。
//!
//! ## 为什么必须只有一份
//!
//! 构图函数建 atom 用它，019 的按需重建走的是同一个构图函数、因此也是同一份默认值。
//! 分成两份的那一刻，undo 路径重建出来的 atom 就会和正常创建出来的不一样——而那条
//! 路径只有「长会话 + 逐出 + undo」三件事同时发生才走得到，通常是在线上。
//!
//! ## 每条默认值都是一个判断，不是零值
//!
//! 这个文件里绝大多数注释在回答同一个问题：**「默认成别的会怎样」**。
//! `ToolsAllowed` 默认成「活着」会让 despawn 掉的子 agent 在 undo 路径上复活；
//! `DisabledBuiltins` 默认成非空会让一个从没提过要求的会话偷偷少几个工具；
//! `Summaries` 默认成非空会给一个从没压过的会话平添一份摘要。三者都是链通、值错、
//! 不报错——本仓最恨的那一类。所以新增槽位时，「它的默认值是什么」要在这里连同
//! 理由一起写下来。

use crate::engine::state::{DEFAULT_MAX_RETRIES, DEFAULT_MAX_TURNS, TurnStatus};
use crate::value::atom_value::AgentValue;
use crate::value::{
    host_prefix, inbox, prefix_chunks, send_plan::SendPlan, send_plan_codec, summaries,
};

use super::atom_key::{AtomKey, ToolCallSlot};
use super::slot::Slot;

impl AtomKey {
    /// 这个键「没有值」的时候是什么。见模块文档「为什么必须只有一份」。
    pub fn default_value(&self) -> AgentValue {
        match self {
            AtomKey::Agent(_, slot) => slot.default_value(),
            AtomKey::ToolCall(_, _, ToolCallSlot::Result) => AgentValue::Pending,
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
            Slot::SkillsActive => {
                AgentValue::Json(std::sync::Arc::new(serde_json::Value::Array(Vec::new())))
            }
            // 「这个会话没有任何注入」= 空数组，同 `SkillsActive` 那条理由：槽位
            // 永远持一个数组，读取点不必区分「空」和「类型错」。**默认值必须是空**
            // ——019 的按需重建拿的就是它，若默认成别的，undo 路径上凭空重建出来的
            // atom 会给一个从没声明过的会话平添几个工具，而工具表在 prompt 最前面。
            Slot::HostTools => {
                AgentValue::Json(std::sync::Arc::new(serde_json::Value::Array(Vec::new())))
            }
            // 同 `HostTools`：空数组而不是 `Null`。默认值必须是空——019 的按需重建
            // 拿的就是它，若默认成别的，undo 路径上凭空重建出来的 atom 会给一个从没
            // 声明过的会话平添几行常驻索引，而索引跟工具表一样在 prompt 最前面。
            Slot::HostSkills => {
                AgentValue::Json(std::sync::Arc::new(serde_json::Value::Array(Vec::new())))
            }
            // 「一个内置工具都没关」= 空数组，同上两条理由。这一条的默认值格外要紧：
            // 它是**减法**，默认成非空就等于给一个从没提过要求的会话偷偷少几个工具，
            // 而少掉的那些模型压根不知道存在过——查起来没有任何线索。
            Slot::DisabledBuiltins => {
                AgentValue::Json(std::sync::Arc::new(serde_json::Value::Array(Vec::new())))
            }
            // 旧快照没有这个键时必须保持可恢复。Null 不代表选择了默认 provider；
            // 如何解释 legacy/default 由 runtime 决定，core 不做能力或路由判断。
            Slot::ExecutionProfile => AgentValue::Null,
            // 恒等元的编码，不是 `Null`：`SendPlan` 每个状态（含 pristine）都走
            // 同一条 `send_plan_codec` 路径，读取点不必区分「没写过」和「写过一个
            // 空的」——它们本来就是同一个值。
            Slot::SendPlan => send_plan_codec::to_value(&SendPlan::new()),
            // 同 `SendPlan`：pristine 编码，不是 `Null`。第一轮之前「没发过请求」
            // 与「上一次发的是 pristine 计划」是同一个值。
            Slot::PrevSendPlan => send_plan_codec::to_value(&SendPlan::new()),
            // 空库的编码，不是 `Null`：**默认值必须是空**——019 的按需重建拿的就是
            // 它，若默认成别的，undo 路径上凭空重建出来的 atom 会给一个从没压过的
            // 会话平添一份摘要正文。同 `SendPlan`，空库也走同一条编解码路径。
            Slot::Summaries => summaries::to_value(&[]),
            // 空列表的编码，不是 `Null`：**默认值必须是空**——019 的按需重建拿的
            // 就是它，若默认成别的，undo 路径上凭空重建出来的 atom 会给一个从没
            // 写过前缀的会话平添几段 system 文本，而那几段在 prompt 最前面
            // （红线 11：整份缓存作废，还不报错）。空列表也走同一条编解码路径，
            // 读取点因此不必区分「没写过」和「写了零块」——它们就是同一个值。
            Slot::PrefixChunks => prefix_chunks::to_value(&[]),
            // `Null` = 不设限（144 追加；语义跟 `ToolsAllowed` 的 `Null` 不同，见
            // `Slot::PrefixAllowed` 文档）。**默认值必须是 `Null`**——019 的按需
            // 重建拿的就是它，若默认成空数组，一个从没被限定过的子 agent 会在
            // undo 路径上凭空多出一份「什么都不给」的名单，而这份名单一旦被 145
            // 拿去过滤组料，就是静默削掉不该削的东西——跟 `ToolsAllowed` 默认成
            // 「活着」是同一类错误，只是这里错的方向反过来。
            Slot::PrefixAllowed => AgentValue::Null,
            // 空列表的编码，不是 `Null`：同 `HostTools`——「没有声明任何开局块」=
            // 空数组，槽位永远持一个数组，读取点不必区分「空」和「类型错」。
            // **默认值必须是空**——019 的按需重建拿的就是它，若默认成别的，undo
            // 路径上凭空重建出来的 atom 会给一个从没声明过的会话平添几行开局块，
            // 而开局块跟工具表一样排在 prompt 最前面（红线 11：整份缓存作废，
            // 还不报错）。
            Slot::HostPrefix => host_prefix::to_value(Vec::new()),
            // 空收件箱的编码，不是 `Null`：**默认值必须是空**——019 的按需重建拿的
            // 就是它，若默认成别的，undo 路径上凭空重建出来的 atom 会给一个从没
            // 收到过消息的 agent 平添几句话，而那几句话会被排空进它的 `Messages`、
            // 从此每一轮都进 prompt（红线 11）。链通、值错、不报错。
            // 空列表也走同一条编解码路径，读取点因此不必区分「没收到过」和
            // 「收到过但已经排空了」——它们就是同一个值。
            Slot::Inbox => inbox::to_value(&[]),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::{AgentId, ToolCallId};

    /// 每个槽位都答得出一个默认值（`match` 穷举保证），且**没有一个是 `Pending`**
    /// ——`Pending` 是「还在等」，是工具槽在飞时的值；一个 agent 槽位默认成它，
    /// 汇聚型 derived 会在一个什么都没发生的会话上永远收敛不了。
    #[test]
    fn every_agent_slot_has_a_non_pending_default() {
        for slot in Slot::ALL {
            assert!(
                !slot.default_value().is_pending(),
                "{slot:?} 默认成了 Pending"
            );
        }
    }

    /// `AtomKey::default_value` 对 agent 键就是槽位的那一份——**同一份**，
    /// 不是抄一遍（模块文档：分成两份的那一刻 undo 路径就会不一样）。
    #[test]
    fn an_agent_key_delegates_to_its_slot() {
        for slot in Slot::ALL {
            assert_eq!(
                AtomKey::Agent(AgentId::root(), slot).default_value(),
                slot.default_value()
            );
        }
    }

    /// 工具调用槽相反：在飞时就该是 `Pending`（`tools_converged` 靠它短路）。
    #[test]
    fn a_tool_call_slot_defaults_to_pending() {
        let key = AtomKey::ToolCall(
            AgentId::root(),
            ToolCallId::new("call_1"),
            ToolCallSlot::Result,
        );
        assert!(key.default_value().is_pending());
    }
}
