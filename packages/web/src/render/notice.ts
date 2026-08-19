// 唯一职责：除了文本流、工具卡片、guard、undo/redo 之外的其余帧类型——
// `notice`/`preflight_drift_alert`/`transport_trouble`/`tool_call_started`/
// `gap`/`lagged`/`session_died`,全部是「一行小字」量级的通报,放在一个文件里
// 而不是拆七个是因为它们共享同一种渲染形状（一行文字 + 一个状态色），
// 拆开反而制造七个几乎相同的文件（`one-file-one-thing` skill「假拆分」判据）。
import type { AgentId, AutoTurnHold, DriftVerdict, Notice, OrphanFate } from "@agent/protocol";

import { appendToTimeline, el, shortAgentLabel } from "../dom";

export function renderNotice(payload: Notice, agent: AgentId): void {
  appendToTimeline(el("div", "notice-line", describeNotice(payload)), agent);
}

function describeNotice(payload: Notice): string {
  // 105 的两条压缩通报是**无字段变体**（闸放行没放行，只有 core 知道；正文多长
  // 宿主自己知道，盖住了哪一段等 107 写进状态之后从状态读），所以它们在 TS 这边
  // 是字符串成员而不是对象——先把它们摘出去，下面的 `in` 才有对象可查。
  if (payload === "CompactionSummaryReceived") return "压缩：摘要已接受";
  if (payload === "CompactionFailed") return "压缩：这一次没做成，历史边界不动";
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

/** 206：轮末还有话没被读到——有人给这个 agent 发了 `srv:agent/send`，而它在这一轮里
 * 再也没有组装过 provider 请求（多半是发的时候它已经答完了）。
 *
 * **编排失误的信号，不是错误**：轮次结果照旧。`next_turn` 的留言不算在里面——那些
 * 本来就该留到下一轮。载荷是事实不是句子（只有 `count`），措辞在这里组，CLI 那份在
 * `agent-cli::print::events`，跟 `renderOrphanedChild` 是同一条规矩。 */
export function renderUnreadMessages(target: AgentId, count: number, agent: AgentId): void {
  const line = `⚠ ${shortAgentLabel(target)} 还有 ${count} 条消息没看到——发的时候它多半已经答完了`;
  appendToTimeline(el("div", "warn-line", line), agent);
}

/** 211：这一轮是**留言自己开的**，不是人开的。
 *
 * 这条要显眼——本仓第一次在没有用户输入的情况下继续烧 token，用户失去的第一样东西
 * 是「我知道现在在干什么」。剩余预算一并给出：那是它还会自己跑几轮的上界。
 *
 * **浏览器里这条比在 CLI 上更要紧**：那儿没有 Ctrl-C，页面的停止按钮是唯一的出口。 */
export function renderAutoTurnStarted(remaining: number, agent: AgentId): void {
  const line = `⟳ 这一轮是留言自己开的（不是你），之后还能自己开 ${remaining} 轮——随时可以停，剩下的留言不会丢`;
  appendToTimeline(el("div", "notice-line", line), agent);
}

/** 211：有留言等着，但这一轮没有自己开。三种成因都不是错误。
 *
 * 三句话都要说清同一件事：**留言没丢**，只是这一轮没人替你处理它。不说这句，
 * 用户读到「还有 3 条留言」只会以为它们被吞了。 */
export function renderAutoTurnHeld(
  pending: number,
  reason: AutoTurnHold,
  agent: AgentId,
): void {
  const line = `⟳ 还有 ${pending} 条留言没处理：${describeAutoTurnHold(reason)}`;
  appendToTimeline(el("div", "notice-line", line), agent);
}

function describeAutoTurnHold(reason: AutoTurnHold): string {
  if (reason.type === "budget_exhausted") {
    return "自驱动预算用完了。留言还在，你说句话它就会被读到（说话也把预算加满）。";
  }
  if (reason.type === "cancelled") {
    return "你喊了停。已经跑完的那几轮不算失败，剩下的留言还在收件箱里。";
  }
  return "刚从上次崩溃恢复出来——恢复不自动往下跑（不然打开就开始烧钱，而你还没看上一轮发生了什么）。留言还在。";
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
