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

  // `auto_turn_held` 的三种成因 → 人话。**三句都要说清同一件事：留言没丢**，
  // 只是这一轮没人替你处理它。不说这句，用户读到「还有 3 条留言」只会以为
  // 它们被吞了。标签取的是 `events.rs` 里 `hold_reason_tag` 那组（跟 server
  // 形态的 serde snake_case 同一组词）。
  function holdReason(reason) {
    if (reason === "budget_exhausted")
      return "自驱动预算用完了。留言还在，你说句话它就会被读到（说话也把预算加满）。";
    if (reason === "cancelled")
      return "你按了 Cancel。已经跑完的那几轮不算失败，剩下的留言还在收件箱里。";
    if (reason === "recovered")
      return "刚从上次崩溃恢复出来——恢复不自动往下跑（不然打开页面就开始烧钱，而你还没看上一轮发生了什么）。留言还在。";
    return reason;
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
        note(`→ calling host tool ${ev.tool}  input=${JSON.stringify(ev.input)}`);
        break;
      case "tool_executed":
        note(`← ${ev.tool} returned ${ev.output_len} bytes${ev.is_error ? " (error)" : ""}`);
        break;
      case "turn_guard":
        log(`[usage] prompt=${ev.prompt_tokens} completion=${ev.completion_tokens} cached=${ev.cached_tokens}`);
        break;
      case "text_delta_end":
        break;
      // M20（决策 35）。这四条**必须显式画出来**，不能落进下面那个 default：
      // 它只会打一行 `[auto_turn_started] `——事件名后面什么都没有，因为这几条
      // 的载荷字段叫 remaining/pending/count，不叫 detail/status/name。
      //
      // 自驱动那两条尤其要紧：**浏览器里没有 Ctrl-C**，用户唯一的出口是上面那个
      // Cancel 按钮，而他得先知道「现在跑的这一轮不是我开的」才会去按它。
      case "auto_turn_started":
        note(`⟳ 这一轮是留言自己开的（不是你），之后还能自己开 ${ev.remaining} 轮——按 Cancel 随时停，剩下的留言不会丢`);
        break;
      case "auto_turn_held":
        note(`⟳ 还有 ${ev.pending} 条留言没处理：${holdReason(ev.reason)}`);
        break;
      case "unread_messages":
        note(`⚠ ${ev.target} 还有 ${ev.count} 条消息没看到——发的时候它多半已经答完了`);
        break;
      case "orphaned_child":
        note(`⚠ 后台子 agent ${ev.child} 没人领：${ev.detail}`);
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
