//! `AgentEvent` → 页面 JS 收得到的一条 JSON。
//!
//! `RunnerCtx::with_agent_events` 收的是一条 `FnMut(AgentEvent)`——CLI 拿它打
//! 终端、server 拿它推 SSE，浏览器宿主拿它调页面给的回调。这个文件就是最后那
//! 一步的翻译表。
//!
//! # 为什么是 JSON 字符串而不是 `js_sys::Object`
//!
//! 事件在流式增量上是**每几十毫秒一条**的热路径。`Reflect::set` 逐字段建对象
//! 要跨 wasm↔JS 边界好几次；序列化成一个字符串只跨一次，页面那边一句
//! `JSON.parse` 就还原了。代价是多一次序列化+解析，收益是边界穿越次数从
//! 「字段数」降到 1。
//!
//! # 变体的取舍：要驱动 UI 的结构化，其余落 `Debug`
//!
//! 增量、工具调用这些页面真要按字段渲染的，逐字段给出来；判读报告 / 通报 /
//! 孤儿告警这类「打出来给人看一眼」的，落 `format!("{:?}")` 的一个 `detail`
//! 字段。**措辞由看的人组**（`RunnerEvent::OrphanedChild` 的文档立的规矩）——
//! 这个宿主目前的「看的人」就是一个调试页面，它要的是能读，不是好看。

use agent_core::Notice;
use agent_runtime::{AgentEvent, RunnerEvent};
use serde_json::{Value, json};

/// 一条事件的 JSON 文本。`agent` 字段恒在——多 agent 并行输出时它是分得清谁
/// 说的唯一凭据（029 §事件归属）。
pub(crate) fn to_json(event: &AgentEvent) -> String {
    let mut payload = body(&event.event);
    if let Value::Object(map) = &mut payload {
        map.insert("agent".to_string(), json!(event.agent.as_str()));
    }
    payload.to_string()
}

fn body(event: &RunnerEvent) -> Value {
    match event {
        RunnerEvent::TextDelta(text) => json!({ "type": "text_delta", "text": &**text }),
        RunnerEvent::ThinkingDelta(text) => json!({ "type": "thinking_delta", "text": &**text }),
        RunnerEvent::ToolCallStarted { name } => {
            json!({ "type": "tool_call_started", "name": &**name })
        }
        RunnerEvent::ToolExecuting { call_id, request } => json!({
            "type": "tool_executing",
            "call_id": &*call_id.0,
            "tool": &*request.tool,
            "input": &*request.input,
        }),
        RunnerEvent::ToolExecuted {
            call_id,
            tool,
            output_len,
            is_error,
        } => json!({
            "type": "tool_executed",
            "call_id": &*call_id.0,
            "tool": &**tool,
            "output_len": output_len,
            "is_error": is_error,
        }),
        RunnerEvent::TurnGuard {
            usage,
            report,
            adjustments,
        } => json!({
            "type": "turn_guard",
            "prompt_tokens": usage.prompt,
            "completion_tokens": usage.completion,
            // `None`（这家不报缓存）与 `Some(0)`（报了没命中）语义不同，
            // `TokenUsage::cached` 的文档写死了这条——JSON 里也照样分成
            // `null` 和 `0`，不揉成一个默认值。
            "cached_tokens": usage.cached,
            "detail": format!("{report:?}"),
            "adjustments": format!("{adjustments:?}"),
        }),
        RunnerEvent::PreflightDriftAlert(verdict) => json!({
            "type": "drift_alert",
            "detail": format!("{verdict:?}"),
        }),
        RunnerEvent::TransportTrouble(text) => {
            json!({ "type": "transport_trouble", "detail": &**text })
        }
        RunnerEvent::Notice(notice) => notice_json(notice),
        RunnerEvent::OrphanedChild { child, fate } => json!({
            "type": "orphaned_child",
            "child": child.as_str(),
            "detail": format!("{fate:?}"),
        }),
        // 206：轮末还有话没被读到。**逐字段给**而不是落 `Debug`——载荷本来就只有
        // 两样事实（谁、几条），`format!("{:?}")` 在这里只会把它包成一句更难读的话。
        RunnerEvent::UnreadMessages { agent, count } => json!({
            "type": "unread_messages",
            "target": agent.as_str(),
            "count": count,
        }),
        // 109（M12）：压缩点在时间线上的两条可见信号。**只报「发生了」，不带
        // 正文**——摘要原文与被清掉的工具结果原文都不在事件里（server 形态下
        // 它们走 `GET /sessions/{id}/compaction_record`；这个宿主还没有对应的
        // 读取口，页面目前只把它们当标记显示）。`turn_id` 原样带出去，页面靠
        // 它把标记跟 undo/redo 对上号。
        RunnerEvent::CompactionApplied {
            turn_id,
            upto,
            summary_id,
        } => json!({
            "type": "compaction_applied",
            "turn_id": turn_id,
            "upto": upto,
            "summary_id": format!("{summary_id:?}"),
        }),
        RunnerEvent::ToolResultsCleared { turn_id, call_ids } => json!({
            "type": "tool_results_cleared",
            "turn_id": turn_id,
            "call_ids": call_ids.iter().map(|id| &*id.0).collect::<Vec<_>>(),
        }),
    }
}

/// loop 自己发的通报。单独拎出来只为把**轮次状态变化**结构化——页面靠它知道
/// 「这一轮真的完了」，其余通报落 `detail`。
fn notice_json(notice: &Notice) -> Value {
    match notice {
        Notice::TurnStatusChanged { status } => json!({
            "type": "turn_status",
            "status": format!("{status:?}"),
        }),
        other => json!({
            "type": "notice",
            "detail": format!("{other:?}"),
        }),
    }
}
