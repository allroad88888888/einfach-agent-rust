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
  return `${request.location} · ${describeReversibility(request)} · ${JSON.stringify(request.input)}`;
}

/** 202（决策 199 §七「承诺挡，事实不挡」）：`reversibility` 是**声明方的自我描述**，
 * 而有一格描述的是一个本仓兑现不了的**承诺**——执行体在别的进程里（宿主
 * `web:`/`desk:`，或 `mcp:`）却声明 `Reversible`「有补偿动作」。那个补偿动作没有
 * 任何人会执行，`/undo` 撞上它会停下来问（`undo_blocked` 的 `no_hook`）。卡片上
 * 只印一个孤零零的 `Reversible`，用户读到的是「这条撤得掉」——**那正是 199 要修
 * 的骗人的那个字。**
 *
 * **其余每一格只印枚举名**，包括同样够不着的 `Pure`：它声明的是「没碰外部世界」
 * 这个**事实**，不需要任何函数来兑现，本来就不挡 undo；给它挂一句「本仓不代为
 * 补偿」等于回答一个没人问过的问题，还会让人以为撤销出了问题。
 *
 * 判据跟 Rust 侧行为面是同一条（`agent_runtime::is_unkeepable_promise`）：声明
 * `Reversible` **且**执行体不在服务端进程（`location !== "Server"`，或名字是
 * `mcp:`——它的 `location` 是 `Server`，子进程往返在宿主本地跑完，可执行体在
 * server 那边）。两边各写一遍是不得已（一边 Rust 一边 TS），所以两边的注释都指向
 * 同一条决策，改一边就得改另一边。 */
function describeReversibility(request: ToolCallRequest): string {
  const outOfProcess = request.location !== "Server" || request.tool.startsWith("mcp:");
  const unkeepablePromise = request.reversibility === "Reversible" && outOfProcess;
  return unkeepablePromise ? `${request.reversibility}（声明，本仓不代为补偿）` : request.reversibility;
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
