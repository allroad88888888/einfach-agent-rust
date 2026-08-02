//! [`Frame`]：SSE 帧 data 的信封（034，033 上报的缺口 1）。
//!
//! # 为什么外面包一层，不是给 [`super::SessionEvent`] 十四个变体各加一个 `agent` 字段
//!
//! 归属是**这条事件的元数据**，不是任何一个变体的载荷——029 判断 5 定的那条
//! 理由（`agent_runtime::AgentEvent` 就是那样包 `RunnerEvent` 的）对
//! `SessionEvent` 同样成立：包一层之后「每条事件都有归属」是类型事实，不是
//! 「加第十五个变体时记得也加上」的纪律。`SessionEvent` 本身因此一个字段没动，
//! `agent_runtime::RunnerEvent → SessionEvent` 的翻译线（`super` 模块的
//! `From<RunnerEvent>`）也不用被迫改形状。
//!
//! # `agent` 从哪来
//!
//! `agent_runtime::RunnerCtx::with_agent_events` 的回调收到的
//! `agent_runtime::AgentEvent { agent, event }` 直接拆出 `agent` 字段
//! （`crate::actor::body`）；`Undo`/`Redo`/`SessionDied`/`TransportTrouble`/
//! `Gap` 这类不是从 runner 泵里发出来的事件（`/undo` 命令的结果、actor 自己的
//! 传输/崩溃通报、031 重连补发合成的缺口帧）一律标 [`agent_core::AgentId::root`]
//! ——它们是会话/连接级的事实，不属于树上任何一个具体 agent 的 `step`。

use serde::{Deserialize, Serialize};

use agent_core::AgentId;

use super::SessionEvent;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct Frame {
    /// 这件事出自哪个 agent 的 `step`/流。034：`ts` feature 门后面导出 TS，
    /// 单字段元组结构体的 `AgentId` 落成裸的 `type AgentId = string`（跟
    /// `ToolCallId` 同一个映射）。
    pub agent: AgentId,
    pub event: SessionEvent,
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;

    /// 红线 3 精神的直接实检：`Frame` 真的过一遍 serde，且形状是
    /// `{"agent":...,"event":{"type":...,"data":...}}`——SSE 下行每一帧的
    /// data 字段最终就长这样（`crate::http::routes::sse::to_sse_event`）。
    #[test]
    fn frame_serializes_as_agent_plus_the_adjacently_tagged_event() {
        let frame = Frame { agent: AgentId::root(), event: SessionEvent::TextDelta(Arc::from("hi")) };
        let json = serde_json::to_value(&frame).unwrap();
        assert_eq!(json["agent"], "root");
        assert_eq!(json["event"]["type"], "text_delta");
        assert_eq!(json["event"]["data"], "hi");

        let round_tripped: Frame = serde_json::from_value(json).unwrap();
        assert_eq!(round_tripped, frame);
    }

    /// 非 root agent 的字符串形——`AgentId::child` 拼出来的路径原样过 serde，
    /// 不是只有 `"root"` 这一个字面量能用。
    #[test]
    fn a_child_agent_serializes_as_its_path_string() {
        let frame = Frame { agent: AgentId::root().child(1), event: SessionEvent::Lagged { skipped: 2 } };
        let json = serde_json::to_value(&frame).unwrap();
        assert_eq!(json["agent"], "root/a1");
    }
}
