// 唯一职责：MCP 各方法的 params 构造与 result 解析——`initialize`、`tools/list`、
// `tools/call`。信封在 `jsonrpc.ts`，翻译成注入声明在 `translate.ts`。
// 逐条对应 `crates/agent-mcp/src/protocol.rs`，规矩照搬：
//
// **未知字段一律忽略、不报错**（协议要向前兼容，server 可能带我们还不认识的
// 字段）——但**该有的字段缺了要报**（`tools/list` 的 result 没有 `tools` 数组
// 是 `unexpected_shape`）。

import type { CapabilityTool } from "@agent/protocol";

import { McpProtocolError } from "./errors";

/** MCP `inputSchema` 的静态类型**借**注入声明那一个字段
 * （生成物里的 `JsonValue`）——它就是被原样搬过去的那份 JSON Schema，
 * 借用比另定一个更不容易漂。 */
export type InputSchema = CapabilityTool["schema"];

/** 本客户端在握手时向 server 声明的**首选**协议版本。跟 Rust 侧
 * `CLIENT_PROTOCOL_VERSION` 保持同一个取值——两边连的是同一批 server，
 * 没有理由提不同的版本。
 *
 * **server 回的版本可能不同，那不是错误**：真 server 会在 client 提议的版本
 * 落在它支持的范围内时原样回显（M6 实测），也可能回一个自己选的。采用哪个
 * 由 `client.ts` 按 server 回值记下来，不做相等断言。 */
export const CLIENT_PROTOCOL_VERSION = "2025-06-18";

/** `initialize` 的 result：协议协商的回值。 */
export interface InitializeResult {
  /** server 决定采用的协议版本（**可能不同于** [`CLIENT_PROTOCOL_VERSION`]）。 */
  protocolVersion: string;
  /** server 声明的能力（`tools`/`resources`/`prompts` 等），原样搬。 */
  capabilities: unknown;
  /** server 自报的名字（`serverInfo.name`），可能缺。 */
  serverName: string | null;
}

/** 工具的行为提示（MCP wire 字段 `annotations`）。这里只关心 `readOnlyHint`
 * ——它决定可逆性（见 `translate.ts`）。其余提示位不解析进来，免得留没人读
 * 的字段。 */
export interface Annotations {
  /** `readOnlyHint`：为 `true` 才翻译成 `"pure"`。缺失或 `false` 都保守落
   * `"irreversible"`。 */
  readOnlyHint: boolean | null;
}

/** server 声明的一个工具（`tools/list` result 里的一项）。 */
export interface McpTool {
  name: string;
  description: string | null;
  /** JSON Schema（MCP wire 字段 `inputSchema`），原样搬进注入声明的 `schema`。
   * 解析时要求它**是个 object**：MCP 规定 `inputSchema` 必须是
   * `type: "object"` 的 JSON Schema——在解析这一步就要求，比一路传到
   * `JSON.stringify` 之前才发现要好。 */
  inputSchema: InputSchema;
  annotations: Annotations | null;
}

/** 构造 `initialize` 的 params（client 首选协议版本 + 能力 + clientInfo）。 */
export function initializeParams(clientName: string, clientVersion: string): Record<string, unknown> {
  return {
    protocolVersion: CLIENT_PROTOCOL_VERSION,
    capabilities: {},
    clientInfo: { name: clientName, version: clientVersion },
  };
}

/** 解析 `initialize` 的 result。未知字段忽略；`protocolVersion` 缺失 →
 * `unexpected_shape`。 */
export function parseInitializeResult(result: unknown): InitializeResult {
  const obj = asObject(result, "initialize result 不是 object");
  const protocolVersion = obj.protocolVersion;
  if (typeof protocolVersion !== "string") {
    throw new McpProtocolError("unexpected_shape", "initialize result 缺 protocolVersion");
  }

  const serverInfo = isObject(obj.serverInfo) ? obj.serverInfo : null;
  const serverName = serverInfo !== null && typeof serverInfo.name === "string" ? serverInfo.name : null;

  // capabilities 缺失落 `null`——跟「server 显式声明空能力 `{}`」区分开，
  // 后者是合法值原样搬。
  return { protocolVersion, capabilities: "capabilities" in obj ? obj.capabilities : null, serverName };
}

/** 解析 `tools/list` 的 result → 工具列表，**顺序原样保留**（进 prompt 的东西
 * 逐字节确定，红线 11 的精神；这边不排序，交给 server 侧 062 按名字排）。 */
export function parseToolsList(result: unknown): McpTool[] {
  const obj = asObject(result, "tools/list result 不是 object");
  const tools = obj.tools;
  if (!Array.isArray(tools)) {
    throw new McpProtocolError("unexpected_shape", "tools/list result 缺 tools 数组");
  }
  return tools.map((item, index) => parseOneTool(index, item));
}

/** 解析 `tools/list` result 里的一项。`name`/`inputSchema` 缺了报
 * `unexpected_shape`（下标写进消息方便定位是第几个工具坏了）；
 * `description`/`annotations` 缺失或类型不对一律当成没有，不报错——它们本来
 * 就是可选提示位。 */
function parseOneTool(index: number, item: unknown): McpTool {
  const obj = asObject(item, `tools[${index}] 不是 object`);

  const name = obj.name;
  if (typeof name !== "string") {
    throw new McpProtocolError("unexpected_shape", `tools[${index}] 缺 name`);
  }
  if (!isObject(obj.inputSchema)) {
    throw new McpProtocolError("unexpected_shape", `tools[${index}] 缺 inputSchema，或者它不是一个 object`);
  }

  const rawAnnotations = isObject(obj.annotations) ? obj.annotations : null;
  const annotations: Annotations | null =
    rawAnnotations === null
      ? null
      : { readOnlyHint: typeof rawAnnotations.readOnlyHint === "boolean" ? rawAnnotations.readOnlyHint : null };

  return {
    name,
    description: typeof obj.description === "string" ? obj.description : null,
    // 这个 `as` 是安全的：整份 result 来自 `JSON.parse`，凡是 JSON 解出来的
    // 值都是 `JsonValue`；`isObject` 已经把「不是 object」那一档挡在上面了。
    inputSchema: obj.inputSchema as InputSchema,
    annotations,
  };
}

/** 构造 `tools/call` 的 params。`toolName` 是**裸的 MCP 工具名**——不带
 * `web:mcp-<server>/` 前缀，那个前缀是本仓命名，server 不认识（剥前缀在
 * `translate.ts` 的 `parseInjectedToolName`）。 */
export function toolsCallParams(toolName: string, args: unknown): Record<string, unknown> {
  return { name: toolName, arguments: args ?? {} };
}

function isObject(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function asObject(value: unknown, message: string): Record<string, unknown> {
  if (!isObject(value)) throw new McpProtocolError("unexpected_shape", message);
  return value;
}
