// 唯一职责：除了文本流、工具卡片、guard、undo/redo 之外的其余帧类型——
// `notice`/`preflight_drift_alert`/`transport_trouble`/`tool_call_started`/
// `gap`/`lagged`/`session_died`,全部是「一行小字」量级的通报,放在一个文件里
// 而不是拆七个是因为它们共享同一种渲染形状（一行文字 + 一个状态色），
// 拆开反而制造七个几乎相同的文件（`one-file-one-thing` skill「假拆分」判据）。
import type { AgentId, DriftVerdict, Notice, OrphanFate } from "@agent/protocol";

import { appendToTimeline, el, shortAgentLabel } from "../dom";

export function renderNotice(payload: Notice, agent: AgentId): void {
  appendToTimeline(el("div", "notice-line", describeNotice(payload)), agent);
}

function describeNotice(payload: Notice): string {
  if ("TurnStatusChanged" in payload) {
    return `轮状态 → ${describeUnion(payload.TurnStatusChanged.status)}`;
  }
  if ("ToolOutputTruncated" in payload) {
    const { original_bytes, kept_bytes } = payload.ToolOutputTruncated;
    return `工具输出被截断：${original_bytes} → ${kept_bytes} 字节`;
  }
  if ("ProtocolViolation" in payload) {
    return `协议违规（state=${describeUnion(payload.ProtocolViolation.state)}）：${payload.ProtocolViolation.event}`;
  }
  return `重试第 ${payload.Retrying.attempt}/${payload.Retrying.max_retries} 次`;
}

function describeUnion(value: unknown): string {
  if (typeof value === "string") return value;
  const entry = value && typeof value === "object" ? Object.entries(value)[0] : undefined;
  return entry ? `${entry[0]}${JSON.stringify(entry[1])}` : String(value);
}

export function renderDriftAlert(verdict: DriftVerdict, agent: AgentId): void {
  appendToTimeline(el("div", "warn-line", `⚠ 前缀漂移 ${describeUnion(verdict)}`), agent);
}

export function renderTransportTrouble(text: string, agent: AgentId): void {
  appendToTimeline(el("div", "warn-line", `⚠ 传输问题：${text}`), agent);
}

export function renderToolCallStarted(name: string, agent: AgentId): void {
  appendToTimeline(el("div", "pending-line", `⋯ 准备调用 ${name}`), agent);
}

/** 054：轮末孤儿告警——模型开了后台子 agent（`spawn(background=true)`）却没有
 * `srv:agent/collect` 就收尾了。`agent` 是**父**（帧信封给的归属：没领是父的
 * 编排失误），`child` 是出事的那个子。
 *
 * 载荷是事实不是句子（`OrphanFate` 三个变体），措辞在这里组——CLI 那份在
 * `agent-cli::print::events::describe_fate`，两处说同一件事、各按自己的排版说，
 * 跟 `AgentActivity` 在两个壳各有一份呈现是同一条规矩。 */
export function renderOrphanedChild(child: AgentId, fate: OrphanFate, agent: AgentId): void {
  const line = `⚠ 后台子 agent ${shortAgentLabel(child)} ${describeOrphanFate(fate)}`;
  appendToTimeline(el("div", "warn-line", line), agent);
}

function describeOrphanFate(fate: OrphanFate): string {
  if (fate.type === "despawned") {
    const { descendants } = fate.data;
    const tail = descendants === 0 ? "" : `连同它的 ${descendants} 个后代一起`;
    return `还在跑，这一轮收尾时${tail}被拆掉了；它在飞的那次调用回来会被丢弃。`;
  }
  if (fate.type === "kept") {
    return `没能在这一轮收尾时拆掉（${fate.data.reason}），它会以活着的状态留到下一轮。`;
  }
  const { bytes, is_error } = fate.data;
  return `已经${is_error ? "失败收场" : "干完了"}，但这一轮结束前没有人 collect 它，${bytes} 字节的结果被丢弃。`;
}

/** `gap`/`lagged`/`session_died`——031/030 同源的「瞎过要知道自己瞎过」/
 * 崩溃终态,`agent` 恒是 `"root"`（`Frame` 信封里这三个变体的归属规则，见
 * `crates/agent-server/src/event/frame.rs` 模块文档：连接/会话级事实，不属于
 * 树上任何一个具体 agent），仍然原样传下去而不是硬编码——分发点（`dispatch.ts`）
 * 不需要为这三个变体特判「不挂归属」。 */
export function renderGap(skipped: number, agent: AgentId): void {
  appendToTimeline(el("div", "gap-line", `⋯ 掉了 ${skipped} 帧（重连补发跟不上,缓冲已经滚过）`), agent);
}

export function renderLagged(skipped: number, agent: AgentId): void {
  appendToTimeline(el("div", "gap-line", `⋯ 订阅跟丢了 ${skipped} 帧`), agent);
}

export function renderSessionDied(reason: string, agent: AgentId): void {
  appendToTimeline(el("div", "error-line", `✕ session 已终止：${reason}`), agent);
}
