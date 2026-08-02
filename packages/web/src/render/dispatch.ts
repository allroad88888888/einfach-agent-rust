// 唯一职责：一帧 `Frame` → 该调渲染层哪个函数。**这就是 issue 033 决策 2 说
// 的「帧解析 = JSON.parse + 判别联合收窄」**——`switch (frame.event.type)`
// 每个 `case` 里 TS 把 `frame.event` 收窄成对应的变体,不需要、也不手写任何
// 一份协议判别联合的声明。
//
// 034：agent 归属现在是真的——`frame.agent` 直接来自 `crates/agent-server/
// src/event/frame.rs` 的信封,不再是 033 那份「疑似子 agent 活动」的近似
// （原来的 `../spawn-activity` 整个被替掉）。每种事件的渲染函数都收
// `frame.agent`,原样往下传给 `appendToTimeline`（`../dom`）——分发这一层不
// 需要再对 spawn 调用本身特判「算不算子 agent 活动」,归属从协议里如实读出来
// 就是对的。
//
// `createRenderer` 每次调用起一份独立的 `StreamCursor` 状态（不是模块级
// 单例）——一个 session 一份,不同 session 互不干扰,也方便测试用一个新实例
// 而不用担心残留状态。
import type { Frame } from "@agent/protocol";

import { StreamCursor } from "./stream";
import * as tool from "./tool";
import * as notice from "./notice";
import { renderUndoOutcome } from "./undo";
import { turnGuard } from "./guard";

export function createRenderer(sessionId: string): (frame: Frame) => void {
  const stream = new StreamCursor();

  return function dispatch(frame: Frame): void {
    const { agent, event } = frame;

    switch (event.type) {
      case "text_delta":
        stream.append(agent, "text", event.data);
        return;
      case "thinking_delta":
        stream.append(agent, "thinking", event.data);
        return;
      case "tool_call_started":
        stream.interrupt();
        notice.renderToolCallStarted(event.data.name, agent);
        return;
      case "preflight_drift_alert":
        stream.interrupt();
        notice.renderDriftAlert(event.data, agent);
        return;
      case "transport_trouble":
        stream.interrupt();
        notice.renderTransportTrouble(event.data, agent);
        return;
      case "tool_executing":
        stream.interrupt();
        tool.toolExecuting(event.data.call_id, event.data.request, agent);
        return;
      case "tool_executed":
        stream.interrupt();
        tool.toolExecuted(event.data.call_id, event.data.tool, event.data.output_len, event.data.is_error, agent);
        return;
      case "turn_guard":
        stream.interrupt();
        turnGuard(event.data.usage, event.data.report, event.data.adjustments, agent);
        return;
      case "notice":
        stream.interrupt();
        notice.renderNotice(event.data, agent);
        return;
      case "undo":
        stream.interrupt();
        renderUndoOutcome("undo", event.data, sessionId, agent);
        return;
      case "redo":
        stream.interrupt();
        renderUndoOutcome("redo", event.data, sessionId, agent);
        return;
      case "lagged":
        stream.interrupt();
        notice.renderLagged(event.data.skipped, agent);
        return;
      case "session_died":
        stream.interrupt();
        notice.renderSessionDied(event.data.reason, agent);
        return;
      case "gap":
        stream.interrupt();
        notice.renderGap(event.data.skipped, agent);
        return;
    }
  };
}
