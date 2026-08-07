//! 跨模块使用的 id 类型。`message.rs` / `tool.rs` / `engine/` 都要引用它们，放进
//! 其中任何一个都会造成模块间的隐式耦合，所以单独开一个文件（红线 9）。
//!
//! [`AgentId`] 住在隔壁 [`agent`] 子模块：它比这里另外几个 id 多一整套**路径代数**
//! （父/子/深度/祖先判定，028），那是一个独立的抽象——「一个 agent 在树里的地址」
//! ——不该和「一次工具调用叫什么」挤在一个文件里。

mod agent;

pub use agent::{AGENT_PATH_SEP, AgentId};

use std::sync::Arc;

use serde::{Deserialize, Serialize};

/// 一次工具调用的标识，贯穿 `ContentBlock::ToolUse` 请求块和对应的
/// `ContentBlock::ToolResult` 块——两者靠它配对。
///
/// 包一层 `Arc<str>` 而不是 `String`：同一个 id 会被请求块和结果块各存一份，
/// 克隆成本压到指针拷贝（红线 5）。id 的取值来自 provider 侧生成的字符串，
/// 不是本仓铸造的，所以只提供一个透传式的构造函数。
/// `Hash` / `Ord`：026 起它是 `AtomKey::ToolCall` 的一部分，而 `AtomFamily<K>` 要求
/// `K: Eq + Hash`，快照/诊断又要按键排序输出（顺序不定的快照没法逐值比对）。
/// 032：`SessionEvent::ToolExecuting`/`ToolExecuted`/`Notice::ToolOutputTruncated`
/// 的 `call_id` 字段类型，`ts` feature 门后面导出 TS——单字段元组结构体，ts-rs
/// 落成裸的 `type ToolCallId = string`（不是 `{ 0: string }`），`Arc<str>` 到 TS
/// 就是 `string`（`Arc`/`Rc`/`Box` 对 ts-rs 是透明包装）。
///
/// `MessageId`（下面）**没有**这个 derive：032 载荷可达性排查完整走了一遍
/// `SessionEvent`/`Command` 的字段图，它没被任何协议字段引用到——历史目前不在
/// 任何 `SessionEvent` 变体里。晚点这个类型真的进了协议面，照这里的写法加 derive
/// 即可，不需要额外设计。[`AgentId`] 034 起也有这个 derive 了（`agent-server` 的
/// `Frame` 信封把它带进了协议面），见它自己的类型文档。
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct ToolCallId(pub Arc<str>);

impl ToolCallId {
    pub fn new(id: impl Into<Arc<str>>) -> Self {
        Self(id.into())
    }
}

/// 一条消息在历史里的序号。
///
/// 包一层 `u64` 而不是让调用方直接传裸整数：**这个 issue 只定类型**。确定性铸造
/// ——从哪个数开始、undo/redo 时号码怎么处理——是历史 append（009）和快照恢复
/// （010）要解决的问题，过早把铸造规则焊在这里，等那两个 issue 落地大概率要推翻。
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug, Serialize, Deserialize)]
pub struct MessageId(pub u64);

/// 一个 skill 的标识（039）。skill 的**内容**（正文 + 携带的工具）活在 store 外的
/// registry 里，store 里只有「哪些被激活」——那是 `Slot::SkillsActive` 槽位里一个
/// **有序的 id 数组**（STATE-MODEL §「落盘的键必须是 AtomKey」：没有 `Skill` 变体，
/// skill 不是一个 atom key，是某个 `Agent(_, SkillsActive)` 槽位的值的一员）。
///
/// `Arc<str>` 而不是 `String`：同一个 id 会在活名单、命令入参、渲染进 prompt 的
/// 注入内容里各被引用一份，克隆压到指针拷贝（红线 5）。
///
/// `Ord` / `Hash`：活名单要**排序去重**后落盘（红线 11——它会被渲染进 system
/// prompt，顺序一漂前缀缓存就全价），排序要一个全序。
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Serialize, Deserialize)]
pub struct SkillId(pub Arc<str>);

impl SkillId {
    /// 从一个字符串造 id。**落盘/传输的反序列化入口**——活名单里的 id 都是这么
    /// 回来的（存成 JSON 字符串数组）。不校验形状：skill 存不存在是 registry 的事
    /// （在 core 之外，红线 7），core 只管这个 id 在不在活名单里。
    pub fn new(id: impl Into<Arc<str>>) -> Self {
        Self(id.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// 一个已由宿主解析并授权的执行 profile 标识（093）。
///
/// core 只把它当作不透明、可持久化的字符串。它对应哪个 provider、endpoint、
/// model、key、client 或 timeout，全部由 runtime registry 解释，绝不进入 durable
/// 状态。这里也不校验 id 是否存在：恢复后 registry 缺项时应由 runtime fail closed。
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Serialize, Deserialize)]
pub struct ExecutionProfileId(Arc<str>);

impl ExecutionProfileId {
    pub fn new(id: impl Into<Arc<str>>) -> Self {
        Self(id.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub(crate) fn into_inner(self) -> Arc<str> {
        self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// serde 往返：`to_string` → `from_str` → 值相等。
    #[test]
    fn tool_call_id_roundtrip() {
        let id = ToolCallId::new("call_1");
        let s = serde_json::to_string(&id).unwrap();
        assert_eq!(serde_json::from_str::<ToolCallId>(&s).unwrap(), id);
    }

    #[test]
    fn message_id_roundtrip() {
        let mid = MessageId(42);
        let s = serde_json::to_string(&mid).unwrap();
        assert_eq!(serde_json::from_str::<MessageId>(&s).unwrap(), mid);
    }

    #[test]
    fn agent_id_root_roundtrip() {
        let id = AgentId::root();
        let s = serde_json::to_string(&id).unwrap();
        assert_eq!(serde_json::from_str::<AgentId>(&s).unwrap(), id);
        assert_eq!(id.as_str(), "root");
    }

    #[test]
    fn skill_id_roundtrip() {
        let id = SkillId::new("pirate-speak");
        let s = serde_json::to_string(&id).unwrap();
        assert_eq!(serde_json::from_str::<SkillId>(&s).unwrap(), id);
        assert_eq!(id.as_str(), "pirate-speak");
    }

    #[test]
    fn execution_profile_id_roundtrip() {
        let id = ExecutionProfileId::new("worker");
        let s = serde_json::to_string(&id).unwrap();
        assert_eq!(serde_json::from_str::<ExecutionProfileId>(&s).unwrap(), id);
        assert_eq!(id.as_str(), "worker");
    }
}
