// 唯一职责：本模块的三类错误。**未知不猜成成功**是这一层的头号原则——照
// `crates/agent-mcp/src/error.rs` 的判断搬过来：猜错了宿主会把一条畸形/失败
// 响应当成有效结果，最后喂进模型的 loop。
//
// 三类分开是因为**处理方式不同**：传输失败可以重试（这个源暂时不可用，
// 失败隔离把它标成 unavailable），协议畸形不能重试（server 就是不合规），
// server 主动回的 JSON-RPC error 则是**正常的业务失败**（工具不存在、参数
// 不对），要原样报给模型看。

/** 协议畸形的分类。跟 Rust 侧 `ProtocolError` 的四个变体一一对应，
 * 好让断言能区分「不是 JSON」和「形状不对」两回事。 */
export type ProtocolErrorKind =
  /** 根本不是合法 JSON。 */
  | "not_json"
  /** 是 JSON 但不是合法的 JSON-RPC 信封（缺 `jsonrpc`/`id`，或 `id` 类型不对）。 */
  | "not_jsonrpc"
  /** 信封合法但语义畸形：`result` 与 `error` 同时存在、或两者都不存在。 */
  | "malformed"
  /** 某个方法的 result 形状不符（如 `tools/list` 的 result 里没有 `tools` 数组）。 */
  | "unexpected_shape";

/** 解析 JSON-RPC 信封或某个 MCP 方法 result 时的失败。 */
export class McpProtocolError extends Error {
  readonly kind: ProtocolErrorKind;

  constructor(kind: ProtocolErrorKind, message: string) {
    super(`MCP 协议错误(${kind}): ${message}`);
    this.name = "McpProtocolError";
    this.kind = kind;
  }
}

/** 传输层失败：连不上、非 2xx、超时、流被掐断。**这一类才触发失败隔离**
 * （`connect.ts` 把这个源标成 unavailable，其余照常）。 */
export class McpTransportError extends Error {
  /** 原始失败（`fetch` 抛的 `TypeError`/`AbortError` 等），保留给 console 看。 */
  readonly cause?: unknown;

  constructor(message: string, cause?: unknown) {
    super(message);
    this.name = "McpTransportError";
    this.cause = cause;
  }
}

/** server 对某次请求回了 JSON-RPC `error` 对象——**协议是通的**，只是这次
 * 调用失败了。 */
export class McpRpcError extends Error {
  readonly code: number;
  readonly data?: unknown;

  constructor(code: number, message: string, data?: unknown) {
    super(`server 报错 [${code}]: ${message}`);
    this.name = "McpRpcError";
    this.code = code;
    this.data = data;
  }
}
