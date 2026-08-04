// 唯一职责：MCP **Streamable HTTP 传输**的一次往返——把一条 JSON-RPC 消息
// POST 出去，把**属于这次请求**的那条响应捞回来。
//
// # 为什么是 Streamable HTTP，不是 stdio、也不是老的 HTTP+SSE
//
// 形态 B 的前提就是「浏览器自己连」：浏览器**没有子进程**，stdio 在这个 host
// 上根本表达不出来（这也是为什么本模块的配置只有 `url`，没有 `command`——
// 前端交 `command` 让 server 去 spawn 就是被否决的形态 A：RCE/SSRF，
// docs/HOST-CAPABILITIES.md §七）。
//
// 剩下的两种 HTTP 传输里选新的那个：Streamable HTTP（2025-03-26 起）单端点，
// `POST` 一条请求，响应体要么是 `application/json`（一条响应），要么是
// `text/event-stream`（一段流，中间可能夹着 server 插播的通知）。老的
// HTTP+SSE（2024-11-05）要先 `GET /sse` 拿一个 `endpoint` 事件再往别处 POST，
// 两条通道分离、状态更多，不做。
//
// # 应答匹配跟 Rust 侧同一套判断，只是分层位置不同
//
// `crates/agent-mcp/src/client.rs` 的 `await_response` 把匹配放在 client 层，
// 因为 stdio 是一条长连接管道、所有响应混在一起。这里响应就在**本次 POST 的
// 响应体**里，「一次往返」是传输自己的语义，匹配放这层更合适。判断本身一字
// 不改：
//
// - 有 `method` 字段的行 → server 主动发的通知/请求，**跳过继续等**；
// - `id` 不对号的响应 → 防御性跳过（不该出现，但跳过比殃及整次调用安全）；
// - 其余解析失败 → **真的协议错误，直接报出去，不吞**。
//
// # 超时与取消都在这一层（前端要自己扛的那部分复杂度）
//
// 服务端 MCP 有 tokio + 进程树兜底，浏览器什么都没有：一次 `fetch` 挂住就是
// 永远挂住。所以每次往返自带 `AbortController` + 计时器，超时把整条连接
// （含 SSE 流）掐掉。见 `README.md`「代价」一节。

import { McpProtocolError, McpRpcError, McpTransportError } from "./errors";
import { isServerInitiated, parseJsonText, parseResponse, requestEnvelope, notificationEnvelope } from "./jsonrpc";
import type { JsonRpcId } from "./jsonrpc";
import { sseMessages } from "./sse";

const SSE_MIME = "text/event-stream";

export interface HttpTransportOptions {
  /** MCP server 的 Streamable HTTP 端点。 */
  url: string;
  /** 附加请求头（鉴权等）。`Accept`/`Content-Type` 由本模块自己定，会被覆盖。 */
  headers?: Record<string, string>;
  /** 注入 `fetch`——只为测试；不传就用全局的。 */
  fetchImpl?: typeof fetch;
}

export class HttpTransport {
  readonly url: string;
  private readonly extraHeaders: Record<string, string>;
  private readonly fetchImpl: typeof fetch;
  private nextId: JsonRpcId = 1;

  /** server 在 `initialize` 响应头里发的 `Mcp-Session-Id`（可能没有——有状态
   * 会话是可选的）。有就必须在后续每个请求上回带。 */
  sessionId: string | null = null;
  /** 协商后的协议版本，握手完由 `client.ts` 写进来，之后随请求头发出去
   * （2025-06-18 起 server 用它判断该按哪一版说话）。 */
  protocolVersion: string | null = null;

  constructor(options: HttpTransportOptions) {
    this.url = options.url;
    this.extraHeaders = { ...options.headers };
    this.fetchImpl = options.fetchImpl ?? globalThis.fetch.bind(globalThis);
  }

  /** 发一条 request，等它那条响应。server 回 JSON-RPC `error` → 抛
   * [`McpRpcError`]（协议是通的，只是这次调用失败）。 */
  async request(method: string, params: unknown, timeoutMs: number): Promise<unknown> {
    const id = this.nextId++;
    return await this.send(requestEnvelope(id, method, params), timeoutMs, id);
  }

  /** 发一条 notification（无 `id`，server 不回响应，按规范回 202）。 */
  async notify(method: string, params: unknown, timeoutMs: number): Promise<void> {
    await this.send(notificationEnvelope(method, params), timeoutMs, null);
  }

  /** 按规范用 `DELETE` 显式结束会话。**尽力而为**：失败只告警不抛——关连接
   * 失败不该把调用方的收尾流程搞崩。 */
  async terminate(timeoutMs: number): Promise<void> {
    if (this.sessionId === null) return;
    try {
      await this.fetchImpl(this.url, {
        method: "DELETE",
        headers: this.buildHeaders(false),
        signal: AbortSignal.timeout(timeoutMs),
      });
    } catch (cause) {
      console.warn(`[mcp] 结束 ${this.url} 的会话失败（忽略）`, cause);
    }
  }

  private async send(body: Record<string, unknown>, timeoutMs: number, id: JsonRpcId | null): Promise<unknown> {
    const controller = new AbortController();
    const timer = setTimeout(() => controller.abort(), timeoutMs);
    try {
      let res: Response;
      try {
        res = await this.fetchImpl(this.url, {
          method: "POST",
          headers: this.buildHeaders(true),
          body: JSON.stringify(body),
          signal: controller.signal,
        });
      } catch (cause) {
        throw new McpTransportError(this.describeFailure(cause, controller.signal, timeoutMs), cause);
      }

      const session = res.headers.get("mcp-session-id");
      if (session !== null && session !== "") this.sessionId = session;

      if (!res.ok) {
        const detail = await res.text().catch(() => "");
        throw new McpTransportError(`${this.url} 返回 HTTP ${res.status}${detail ? `: ${detail}` : ""}`);
      }

      if (id === null) {
        // notification：规范说 202 + 空 body。body 可能仍在（有些实现回
        // `{}`），读掉释放连接，内容不看。
        await res.text().catch(() => "");
        return null;
      }

      try {
        return await this.readResponse(res, id);
      } catch (cause) {
        if (cause instanceof McpProtocolError || cause instanceof McpRpcError) throw cause;
        throw new McpTransportError(this.describeFailure(cause, controller.signal, timeoutMs), cause);
      }
    } finally {
      clearTimeout(timer);
    }
  }

  /** 从响应体里捞出 `id` 对上的那条响应，跳过插播的通知（见模块注释）。 */
  private async readResponse(res: Response, id: JsonRpcId): Promise<unknown> {
    const contentType = res.headers.get("content-type") ?? "";

    if (contentType.includes(SSE_MIME)) {
      if (res.body === null) throw new McpTransportError(`${this.url} 的 SSE 响应没有 body`);
      for await (const payload of sseMessages(res.body)) {
        const matched = matchResponse(parseJsonText(payload), id);
        if (matched !== null) return matched.result;
      }
      throw new McpProtocolError("malformed", `SSE 流结束了也没等到 id=${id} 的响应`);
    }

    const text = await res.text();
    const parsed = parseJsonText(text);
    // 批量响应（2025-03-26 允许、2025-06-18 去掉）——收得下就收，宽进严出。
    for (const item of Array.isArray(parsed) ? parsed : [parsed]) {
      const matched = matchResponse(item, id);
      if (matched !== null) return matched.result;
    }
    throw new McpProtocolError("malformed", `响应体里没有 id=${id} 的响应`);
  }

  private buildHeaders(withBody: boolean): Record<string, string> {
    const headers: Record<string, string> = { ...this.extraHeaders };
    // 规范要求 client 两种都接受——server 按自己方便挑一种回。
    headers.Accept = "application/json, text/event-stream";
    if (withBody) headers["Content-Type"] = "application/json";
    if (this.sessionId !== null) headers["Mcp-Session-Id"] = this.sessionId;
    if (this.protocolVersion !== null) headers["MCP-Protocol-Version"] = this.protocolVersion;
    return headers;
  }

  /** 把 `fetch`/流读取的失败翻成一句能定位的话。**超时和连不上要分得开**
   * ——前者是 server 太慢，后者是地址/CORS/服务没起，排查方向完全不同。 */
  private describeFailure(cause: unknown, signal: AbortSignal, timeoutMs: number): string {
    if (signal.aborted) return `${this.url} 超时（${timeoutMs}ms）`;
    return `连不上 ${this.url}（${describeCause(cause)}）`;
  }
}

/** `fetch` 失败时真正有用的信息常常埋在 `cause` 里（`TypeError: fetch failed`
 * 的 `cause` 才是 `ECONNREFUSED`/证书错误/CORS）。剥一层再拼进消息——不剥的
 * 话每个连不上的源报出来的都是同一句废话，等于没报。 */
function describeCause(cause: unknown): string {
  if (!(cause instanceof Error)) return String(cause);
  const inner = (cause as { cause?: unknown }).cause;
  const innerText = inner instanceof Error ? `：${inner.message}` : "";
  return `${cause.name}: ${cause.message}${innerText}`;
}

/** 一条消息要么是「不是我们等的」（返回 `null` 继续等），要么就是答案。
 * 答案包一层 `{ result }` 而不是直接返回：`result` 本身可能是 `null`，
 * 裸返回会跟「没匹配上」撞成同一个值。 */
function matchResponse(raw: unknown, id: JsonRpcId): { result: unknown } | null {
  if (isServerInitiated(raw)) return null;
  const response = parseResponse(raw);
  if (response.id !== id) return null;
  if (response.kind === "error") {
    throw new McpRpcError(response.error.code, response.error.message, response.error.data);
  }
  return { result: response.result };
}
