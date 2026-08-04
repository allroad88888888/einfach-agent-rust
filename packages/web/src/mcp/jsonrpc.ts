// 唯一职责：JSON-RPC 2.0 **信封**的构造与解析。MCP 各方法的 result 形状在
// `protocol.ts`，怎么把信封发出去/收回来在 `transport.ts`。
//
// 跟 Rust 侧 `crates/agent-mcp/src/jsonrpc.rs` 的分工完全一致，只有一处刻意
// 的差别：那边编解码的是**字节**（stdio 按换行分帧），这里进出的是**已经
// `JSON.parse` 过的值**——Streamable HTTP 传输的分帧是 HTTP body / SSE 事件，
// 不是换行，把 `JSON.parse` 揉进这一层会让「一条消息」和「一帧」重新耦合。

import { McpProtocolError } from "./errors";

/** 请求 id。本模块只发 `number` id（自增），不发字符串 id——JSON-RPC 允许
 * 两种，少一种形状就少一处要对齐的地方。 */
export type JsonRpcId = number;

/** JSON-RPC 2.0 的 error 对象。 */
export interface JsonRpcErrorObject {
  code: number;
  message: string;
  data?: unknown;
}

/** 一条 JSON-RPC 2.0 响应。notification（无 `id`）不是响应，不在这里。 */
export type RpcResponse =
  | { kind: "result"; id: JsonRpcId; result: unknown }
  | { kind: "error"; id: JsonRpcId; error: JsonRpcErrorObject };

/** 构造一条 request 的信封。`params` 为 `undefined` 时不写 `params` 字段
 * （有些 server 对 `params: null` 敏感）——同 Rust 侧 `encode_request`。 */
export function requestEnvelope(id: JsonRpcId, method: string, params?: unknown): Record<string, unknown> {
  const envelope: Record<string, unknown> = { jsonrpc: "2.0", id, method };
  if (params !== undefined) envelope.params = params;
  return envelope;
}

/** 构造一条 notification 的信封（无 `id`，server 不回响应）。
 * `notifications/initialized` 用它。 */
export function notificationEnvelope(method: string, params?: unknown): Record<string, unknown> {
  const envelope: Record<string, unknown> = { jsonrpc: "2.0", method };
  if (params !== undefined) envelope.params = params;
  return envelope;
}

/** 这条消息是不是 **server 主动发起**的（通知，或 server 发给 client 的
 * 请求）——判据就是「有 `method` 字段」。等响应的循环靠它跳过插播的通知：
 * `crates/agent-mcp/src/client.rs` 的 `await_response` 有完整先例，042 实测
 * 见过 `notifications/tools/list_changed` 抢在 `tools/list` 的响应之前到达。 */
export function isServerInitiated(raw: unknown): boolean {
  return isObject(raw) && "method" in raw;
}

/** 把一段文本解析成 JSON——失败落 `not_json`，不吞。 */
export function parseJsonText(text: string): unknown {
  try {
    return JSON.parse(text) as unknown;
  } catch (cause) {
    throw new McpProtocolError("not_json", `${cause instanceof Error ? cause.message : String(cause)}`);
  }
}

/** 解析一条已经 `JSON.parse` 过的 JSON-RPC 响应。
 *
 * **未知不猜成成功**：缺 `jsonrpc`/`id` → `not_jsonrpc`；`result` 与 `error`
 * 同时在或都不在 → `malformed`。 */
export function parseResponse(raw: unknown): RpcResponse {
  if (!isObject(raw)) throw new McpProtocolError("not_jsonrpc", "顶层不是 JSON object");
  if (raw.jsonrpc !== "2.0") throw new McpProtocolError("not_jsonrpc", `jsonrpc 字段不是 "2.0"`);

  const id = raw.id;
  if (typeof id !== "number" || !Number.isInteger(id)) {
    throw new McpProtocolError("not_jsonrpc", "响应的 id 缺失或不是整数");
  }

  const hasResult = "result" in raw;
  const hasError = "error" in raw;
  if (hasResult === hasError) {
    throw new McpProtocolError("malformed", "result 与 error 必须恰好有一个");
  }
  if (hasResult) return { kind: "result", id, result: raw.result };

  const error = raw.error;
  if (!isObject(error) || typeof error.code !== "number" || typeof error.message !== "string") {
    throw new McpProtocolError("malformed", "error 对象缺 code/message 或类型不对");
  }
  return { kind: "error", id, error: { code: error.code, message: error.message, data: error.data } };
}

function isObject(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}
