//! 打真实 API 的探针共用部件。
//!
//! 这里**不是**产品代码：它的产出是 `probes/results/` 下的事实记录，用来决定
//! `agent-core` 的 `Capabilities` 怎么定。独立 workspace，主工程碰不到。

pub mod caps;
pub mod client;
pub mod config;
pub mod exp;
pub mod fixture;
pub mod http;
