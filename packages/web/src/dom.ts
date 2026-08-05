// 唯一职责：DOM 访问的一个入口——页面上那几个固定元素（timeline/status/输入
// 控件）在这里查一次、导出引用，其余文件不再自己 `document.querySelector`。
// `el`/`appendToTimeline` 是渲染层公用的两个最小 DOM 构造/挂载helper。
import type { AgentId } from "@agent/protocol";

const ROOT_AGENT: AgentId = "root";

function requireEl<T extends Element>(selector: string): T {
  const found = document.querySelector<T>(selector);
  if (!found) {
    throw new Error(`缺少页面元素 ${selector}（index.html 改动没同步？）`);
  }
  return found;
}

export const timeline = requireEl<HTMLElement>("#timeline");
export const statusEl = requireEl<HTMLElement>("#status");
export const inputEl = requireEl<HTMLTextAreaElement>("#input");
export const composerEl = requireEl<HTMLFormElement>("#composer");
export const imageInputEl = requireEl<HTMLInputElement>("#image-input");
export const imageAttachmentsEl = requireEl<HTMLElement>("#image-attachments");
export const imageMessageEl = requireEl<HTMLElement>("#image-message");
export const undoBtn = requireEl<HTMLButtonElement>("#undo-turn");
export const redoBtn = requireEl<HTMLButtonElement>("#redo");
export const cancelBtn = requireEl<HTMLButtonElement>("#cancel");
// 049：活 agent 树面板的挂载点——跟 `#timeline` 并存（issue 049 范围条款 2：
// 树面板答「谁在干啥」，帧流答「说了什么」，互补不替代），`render/agent_tree.ts`
// 是它唯一的写入方，整棵重画（哑渲染器，不做增量 DOM patch）。
export const agentTreeEl = requireEl<HTMLElement>("#agent-tree");

/** 造一个元素，`textContent` 走安全赋值（不是 `innerHTML`）——渲染的每一段
 * 文本都可能含用户输入或模型输出，没有理由让它有机会被当成标签解析。 */
export function el(tag: string, className?: string, text?: string): HTMLElement {
  const node = document.createElement(tag);
  if (className) node.className = className;
  if (text !== undefined) node.textContent = text;
  return node;
}

/** 挂到时间线末尾，滚到底部。`agent` 是这一帧真实的归属（034 起 `Frame.agent`
 * ——不再是「疑似子 agent 活动」的近似，见 `../render/dispatch`）：root 不挂
 * 标记，非 root 挂缩进/变色（`style.css` 的 `.sub-agent`）并带一个短标签
 * （`root/a1/a2` → `a1/a2`，`shortAgentLabel`），好让并行的多个子 agent 在
 * 时间线上分得开,不是只有一个笼统的「有子 agent 在飞」颜色块。 */
export function appendToTimeline(node: HTMLElement, agent: AgentId): void {
  const isSubAgent = agent !== ROOT_AGENT;
  node.classList.toggle("sub-agent", isSubAgent);
  if (isSubAgent) node.dataset.agent = shortAgentLabel(agent);
  timeline.appendChild(node);
  timeline.scrollTop = timeline.scrollHeight;
}

/** `root/a1/a2` → `a1/a2`，root 本身原样返回 `"root"`——去掉 root 那一段，
 * 剩下的路径就是「这是哪个子 agent」（跟 `agent-cli` 的 `print::events` 同一
 * 条判据，029 判断 20；也是 `agent-cli` `print/agent_tree.rs` 的 `short_id`
 * 同一条规则）：`root/a1` 和 `root/a2` 的路径不同,标签自然不同,并行的多个
 * 子 agent 因此分得开,不需要一个额外的「第几个子 agent」计数器。导出给
 * `render/agent_tree.ts` 复用——树面板每一行的短 id 要跟时间线上的子 agent
 * 标签是同一份文本，不重新发明一遍。 */
export function shortAgentLabel(agent: AgentId): string {
  const prefix = `${ROOT_AGENT}/`;
  return agent.startsWith(prefix) ? agent.slice(prefix.length) : agent;
}
