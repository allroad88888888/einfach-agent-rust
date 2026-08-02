// 唯一职责：工具调用卡片——`tool_executing` 起一张卡（名字/参数/
// reversibility），`tool_executed` 按 `call_id` 找回它补上结果（结果长度/
// 是否出错）。`srv:agent/spawn` 有专门的展示（任务描述而不是原始 JSON 参数,
// 虚线边框），因为它的 `input` 形状固定是 `{task, tools?}`
// （`agent_runtime::spawn_tool::spawn_spec`）,直接摊 JSON 可读性不如摘 `task`。
import type { AgentId, ToolCallId, ToolCallRequest } from "@agent/protocol";

import { appendToTimeline, el } from "../dom";

/** spawn 工具全名——`agent_runtime::spawn_tool::SPAWN_TOOL`。034 起这里是它
 * 唯一的消费者（原先 `../spawn-activity` 那份「疑似子 agent 活动」近似整个
 * 被真实的 `Frame.agent` 取代，不再需要单独一个模块）。 */
const SPAWN_TOOL = "srv:agent/spawn";

const cards = new Map<ToolCallId, HTMLElement>();

export function toolExecuting(callId: ToolCallId, request: ToolCallRequest, agent: AgentId): void {
  const isSpawn = request.tool === SPAWN_TOOL;
  const card = el("div", isSpawn ? "tool-card spawn-card running" : "tool-card running");
  card.append(el("div", "tool-title", isSpawn ? "🌱 派生子 agent" : `⚙ ${request.tool}`));
  card.append(el("div", "tool-meta", metaLine(request, isSpawn)));
  cards.set(callId, card);
  appendToTimeline(card, agent);
}

function metaLine(request: ToolCallRequest, isSpawn: boolean): string {
  if (isSpawn) {
    const input = request.input as { task?: unknown } | null;
    const task = typeof input?.task === "string" ? input.task : JSON.stringify(request.input);
    return `task: ${task}`;
  }
  return `${request.location} · ${request.reversibility} · ${JSON.stringify(request.input)}`;
}

export function toolExecuted(callId: ToolCallId, tool: string, outputLen: number, isError: boolean, agent: AgentId): void {
  let card = cards.get(callId);
  if (!card) {
    // `tool_executing` 那一帧被 gap 冲掉、或者压根没收到（重连补发边界）——
    // 不静默丢失这条信息,单独起一张卡片,至少把结果显示出来。
    card = el("div", "tool-card");
    appendToTimeline(card, agent);
    cards.set(callId, card);
  }
  card.classList.remove("running");
  card.classList.add(isError ? "error" : "done");
  card.append(el("div", "tool-result", `${isError ? "✗" : "✓"} ${tool} · 输出 ${outputLen} 字节`));
}
