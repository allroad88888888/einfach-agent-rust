// 唯一职责：把连续到达的 `text_delta`/`thinking_delta` 攒进同一个气泡——
// 同一种 kind **且同一个 agent** 连续到达时复用一个 DOM 节点（增量流式追加,
// issue 033 验收原文），换 kind、换 agent、或者被别的事件类型打断时另起一个,
// 视觉上自然分段。034：agent 归属现在是真的（`Frame.agent`）——两个并行子
// agent 的增量哪怕紧挨着到达也不会被粘进同一个气泡,这是「真分栏」的关键一环
// （只挂视觉标记不够,气泡本身也得按 agent 分开)。thinking 弱化显示靠
// `style.css` 的 `.bubble.thinking`,这里只管挂 class。
import type { AgentId } from "@agent/protocol";

import { appendToTimeline, el } from "../dom";

type StreamKind = "text" | "thinking";

export class StreamCursor {
  private kind: StreamKind | null = null;
  private agent: AgentId | null = null;
  private node: HTMLElement | null = null;

  append(agent: AgentId, kind: StreamKind, chunk: string): void {
    if (this.kind !== kind || this.agent !== agent || this.node === null) {
      this.node = el("p", kind === "thinking" ? "bubble thinking" : "bubble");
      appendToTimeline(this.node, agent);
      this.kind = kind;
      this.agent = agent;
    }
    this.node.textContent += chunk;
  }

  /** 非流式事件（工具调用、通报……）打断了连续增量——下一段增量该另起一个
   * 气泡,不能接在旧的后面,不然「工具调用之前的话」和「工具调用之后的话」
   * 会被粘成一段。 */
  interrupt(): void {
    this.kind = null;
    this.agent = null;
    this.node = null;
  }
}
