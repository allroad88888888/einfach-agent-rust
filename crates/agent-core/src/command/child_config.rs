//! 子 agent 出生时固化的配置。
//!
//! 这里只有能进入 durable core 的值：工具授权快照与不透明执行 profile id。
//! provider、endpoint、model、key、client 等 live binding 由 runtime 持有，core
//! 既不认识也不解析。

use std::sync::Arc;

use crate::ids::ExecutionProfileId;

/// spawn 一个子 agent 时要固化的配置。
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct ChildConfig {
    /// 这个子 agent 被允许使用的工具全名（如 `srv:fs/read`）。
    ///
    /// 这是 spawn 当时的快照；落进槽位前会排序去重，保证 prompt 字节稳定。
    pub tools_allowed: Vec<Arc<str>>,
    /// runtime 已解析并授权的执行 profile id。
    ///
    /// `None` 只用于既有默认 spawn 路径与旧状态兼容；core 不把它解释成某个
    /// provider，也不替 runtime 选择 fallback。
    pub execution_profile: Option<ExecutionProfileId>,
}
