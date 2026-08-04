// 唯一职责：把一段 `text/event-stream` 响应体拆成一条条 `data:` 载荷。
// **不认识 MCP、不认识 JSON-RPC**——它只做 SSE 分帧，谁来消费是 `transport.ts`
// 的事。
//
// 为什么不用 `EventSource`：`EventSource` 只会发 `GET`、不能带自定义头、拿不到
// **POST 响应体**里的流。MCP 的 Streamable HTTP 传输恰恰是「POST 一条请求，
// 响应体可能是一段 SSE 流」，所以这里必须自己读 `Response.body`。
// （`src/connection.ts` 连 agent-server 那条 SSE 用的仍然是原生 `EventSource`，
// 两者场景不同，别混。）
//
// 为什么用 `getReader()` 而不是 `for await (const c of body)`：`ReadableStream`
// 的异步迭代在浏览器里至今没有普遍支持（Chrome 没有），Node 有——写成 reader
// 循环两边都能跑，测试脚本和真浏览器走的是同一段代码。

/** 逐条吐出 SSE 事件的 `data` 载荷（多行 `data:` 按规范用 `\n` 拼接）。
 *
 * 消费方提前 `break`/`return` 会触发这个生成器的 `finally` → `cancel()` 掉
 * 底层流 → 连接及时释放。`transport.ts` 等到自己那条响应就 `return`，靠的
 * 就是这条。 */
export async function* sseMessages(body: ReadableStream<Uint8Array>): AsyncGenerator<string> {
  const reader = body.getReader();
  const decoder = new TextDecoder();
  const pending: string[] = [];
  let buffer = "";

  try {
    for (;;) {
      const { done, value } = await reader.read();
      if (done) break;
      buffer += decoder.decode(value, { stream: true });

      let newline = buffer.indexOf("\n");
      while (newline >= 0) {
        const payload = consumeLine(stripCr(buffer.slice(0, newline)), pending);
        buffer = buffer.slice(newline + 1);
        if (payload !== null) yield payload;
        newline = buffer.indexOf("\n");
      }
    }

    buffer += decoder.decode();
    // 流结束时**攒着的东西要吐出来**：规范说末尾不完整的事件应当丢弃，但
    // 「丢弃」在这里等于静默丢掉一条响应——那正是本仓最不能接受的失败方式。
    // 吐出去顶多是 `not_json`（看得见的错），不吐就是挂到超时（看不见的错）。
    const tail = stripCr(buffer);
    if (tail !== "") {
      const payload = consumeLine(tail, pending);
      if (payload !== null) yield payload;
    }
    if (pending.length > 0) yield pending.join("\n");
  } finally {
    void reader.cancel().catch(() => {
      // 取消失败无所谓——连接要么已经关了，要么马上被 GC；这里再抛会盖住
      // 调用方真正关心的那个错误。
    });
  }
}

/** 处理一行。空行 = 事件边界（把攒着的 `data:` 吐出去）；`:` 开头是注释；
 * 其余按 `field: value` 拆，只认 `data`（MCP 用不到 `event`/`id`/`retry`）。
 * 返回非 `null` 表示一个事件完整了。 */
function consumeLine(line: string, pending: string[]): string | null {
  if (line === "") {
    if (pending.length === 0) return null;
    const payload = pending.join("\n");
    pending.length = 0;
    return payload;
  }
  if (line.startsWith(":")) return null;

  const colon = line.indexOf(":");
  const field = colon < 0 ? line : line.slice(0, colon);
  if (field !== "data") return null;

  let value = colon < 0 ? "" : line.slice(colon + 1);
  if (value.startsWith(" ")) value = value.slice(1);
  pending.push(value);
  return null;
}

function stripCr(line: string): string {
  return line.endsWith("\r") ? line.slice(0, -1) : line;
}
