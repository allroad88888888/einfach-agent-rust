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
//
// 049：`agent_tree` 是唯一不写进时间线的变体——它标的是 `AgentId::root()`
// （会话级快照,不是某个具体 agent 的活动,`event/frame.rs` 的既有约定），
// `event.data`（`AgentTree`）整个甩给 `renderAgentTree`（`./agent_tree`）
// 重画独立的树面板,不经 `frame.agent`/`appendToTimeline` 那条路。
//
// 109：`compaction_applied`/`tool_results_cleared` 进时间线（跟 `notice` 同款
// ——它们标在真实归属的 agent 上）；`undo`/`redo` 帧的 `turn_id`（`applied`
// 分支才有）转给 `compactionTimeline.undo`/`.redo`，精确隐藏/恢复那一轮产生的
// 标记——跟 `userInputs.undo()`/`.redo()` 同一处调用点，同一条「090 的教训」。
import type { Frame } from "@agent/protocol";

import { StreamCursor } from "./stream";
import * as tool from "./tool";
import * as notice from "./notice";
import { renderUndoOutcome } from "./undo";
import { turnGuard } from "./guard";
import { renderAgentTree } from "./agent_tree";
import type { UserInputTimeline } from "./user_input";
import type { CompactionTimeline } from "./compaction";

export function createRenderer(
  sessionId: string,
  userInputs: UserInputTimeline,
  compactionTimeline: CompactionTimeline,
): (frame: Frame) => void {
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
        if (event.data.type === "applied") {
          userInputs.undo();
          compactionTimeline.undo(event.data.data.turn_id);
        }
        renderUndoOutcome("undo", event.data, sessionId, agent);
        return;
      case "redo":
        stream.interrupt();
        if (event.data.type === "applied") {
          userInputs.redo();
          compactionTimeline.redo(event.data.data.turn_id);
        }
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
      case "orphaned_child":
        // 054：轮末孤儿告警。帧的 `agent` 是**父**（没 collect 是父的编排失误），
        // 出事的子在 `event.data.child` 里——所以这一行进的是父的时间线,树面板
        // 那边它已经消失了（被 `despawn_child` 拆掉 → 不在 `live_agents` 里 →
        // 下一帧 `agent_tree` 快照自然没有它，哑渲染不需要为这件事写一行代码）。
        stream.interrupt();
        notice.renderOrphanedChild(event.data.child, event.data.fate, agent);
        return;
      case "unread_messages":
        // 206：轮末未读告警。帧的 `agent` 是收信人自己（那条话是投给它的），
        // `event.data.agent` 是同一个 id——保留字段不硬编码，跟 `orphaned_child`
        // 那条一样：归属规则住在服务端的 `frame.rs`，分发点不替它做判断。
        stream.interrupt();
        notice.renderUnreadMessages(event.data.agent, event.data.count, agent);
        return;
      case "auto_turn_started":
        // 211：这一轮是留言自己开的。打断连续气泡——它是一条独立的通报，
        // 而且是用户最需要一眼看到的那种（会话在没人碰键盘时继续烧 token）。
        stream.interrupt();
        notice.renderAutoTurnStarted(event.data.remaining, agent);
        return;
      case "auto_turn_held":
        // 211：有留言等着但没自己开。三种成因都不是错误，但都得说出来
        // ——不说的话「什么都没发生」跟「留言被吞了」在外面长得一模一样。
        stream.interrupt();
        notice.renderAutoTurnHeld(event.data.pending, event.data.reason, agent);
        return;
      case "agent_tree":
        // 049：树面板跟时间线是两块独立 DOM（`#agent-tree` vs `#timeline`），
        // 不写进时间线,因此不打断 `stream` 的连续增量气泡——树变化和文本流是
        // 两件事,互不打断彼此的连续性。哑渲染：拿到快照就整棵重画
        // （`renderAgentTree`），不在这一层做任何增量判断。
        renderAgentTree(event.data);
        return;
      case "compaction_applied":
        stream.interrupt();
        compactionTimeline.applied(event.data.turn_id, event.data.upto, event.data.summary_id, agent);
        return;
      case "tool_results_cleared":
        stream.interrupt();
        compactionTimeline.cleared(event.data.turn_id, event.data.call_ids, agent);
        return;
    }
  };
}
