// transcript.js —— 对话区与事件流的渲染：runner 事件 / 重放出来的历史 → DOM。
//
// 从 index.html 摘出来是 issue 130 的一部分（红线 9：那个文件加识图那一段之前
// 339 行，已经超了）。摘的这一刀落在「画」这个职责上：本文件只往两个元素里写
// 东西，不认识 AgentHost、不认识工具、不发任何请求，反过来 index.html 也不再
// 直接碰 `#transcript` 的 DOM。
//
// 唯一的内部状态是 `assistantNode`——流式增量要往同一个节点里追加，所以「当前
// 这一轮的 assistant 气泡」必须记住。它跟着渲染逻辑住在这里，不外泄成一个页面
// 级变量：`appendUserTurn` / `renderHistory` / `endTurn` 三个入口各自在该重置的
// 时候重置它，调用方不需要知道有这么个东西。

/**
 * @param {{transcript: HTMLElement, logPre: HTMLElement}} elements
 * @returns {{log: (line: string) => void, onEvent: (json: string) => void,
 *   renderHistory: (entries: Array<object>) => void,
 *   appendUserTurn: (text: string) => void, endTurn: () => void}}
 */
export function mountTranscript({ transcript, logPre }) {
  let assistantNode = null;

  function log(line) {
    logPre.textContent += line + "\n";
    logPre.scrollTop = logPre.scrollHeight;
  }

  function scrollToBottom() {
    transcript.scrollTop = transcript.scrollHeight;
  }

  function turnNode(who) {
    const div = document.createElement("div");
    div.className = "turn";
    const label = document.createElement("div");
    label.className = "who";
    label.textContent = who;
    div.appendChild(label);
    const body = document.createElement("div");
    div.appendChild(body);
    transcript.appendChild(div);
    scrollToBottom();
    return body;
  }

  function note(text) {
    const div = document.createElement("div");
    div.className = "tool";
    div.textContent = text;
    transcript.appendChild(div);
    scrollToBottom();
  }

  // runner 事件 → 页面。形状见 crates/agent-wasm/src/events.rs。
  function onEvent(json) {
    const ev = JSON.parse(json);
    switch (ev.type) {
      case "text_delta":
        if (!assistantNode) assistantNode = turnNode("assistant");
        assistantNode.textContent += ev.text;
        scrollToBottom();
        break;
      case "tool_executing":
        note(`→ 调用宿主工具 ${ev.tool}  input=${JSON.stringify(ev.input)}`);
        break;
      case "tool_executed":
        note(`← ${ev.tool} 返回 ${ev.output_len} 字节${ev.is_error ? "（错误）" : ""}`);
        break;
      case "turn_guard":
        log(`[usage] prompt=${ev.prompt_tokens} completion=${ev.completion_tokens} cached=${ev.cached_tokens}`);
        break;
      case "text_delta_end":
        break;
      default:
        log(`[${ev.type}] ${ev.detail ?? ev.status ?? ev.name ?? ""}`);
    }
  }

  function renderHistory(entries) {
    transcript.textContent = "";
    for (const message of entries) {
      const body = turnNode(message.role);
      for (const block of message.blocks) {
        if (block.kind === "text") body.textContent += block.text;
        else if (block.kind === "tool_use") note(`→ ${block.name} ${JSON.stringify(block.input)}`);
        else if (block.kind === "tool_result") note(`← ${block.content}`);
      }
    }
    assistantNode = null;
  }

  function appendUserTurn(text) {
    turnNode("user").textContent = text;
    assistantNode = null;
  }

  function endTurn() {
    assistantNode = null;
  }

  return { log, onEvent, renderHistory, appendUserTurn, endTurn };
}
