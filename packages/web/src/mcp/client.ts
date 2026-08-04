// 唯一职责：**一个**已握手的 MCP server 连接——`initialize` 握手、
// `tools/list`、`tools/call`。多 server 的装载与失败隔离在 `connect.ts`，
// 一次往返怎么走在 `transport.ts`。对应 `crates/agent-mcp/src/client.rs`。
//
// # 协议版本：协商不是断言
//
// 握手时 client 提议 [`CLIENT_PROTOCOL_VERSION`]，server 回它将采用的版本
// ——**这里不比较两者是否相等**，只把 server 回的记下来
// （[`McpClient.protocolVersion`]）继续走。真 server 会在 client 提议的版本
// 落在它支持的范围内时原样回显（M6 对
// `@modelcontextprotocol/server-everything` 实测过），也可能回一个别的。
// 版本号会随 server 升级漂移，靠协商兜底是显式决策，不是等它稳定下来再改
// 常量。**断言相等 = 每次 server 升级都炸一次，还是白炸。**

import { McpTransportError } from "./errors";
import type { McpTool } from "./protocol";
import {
  CLIENT_PROTOCOL_VERSION,
  initializeParams,
  parseInitializeResult,
  parseToolsList,
  toolsCallParams,
} from "./protocol";
import { HttpTransport } from "./transport";

/** 握手超时。浏览器这边没有 `npx` 首次拉包那种慢路径（那是 stdio 的事，
 * Rust 侧因此给了 60s），远端 HTTP 握手 20s 足够——挂久了不如早点标成
 * unavailable、让会话先建起来。 */
export const DEFAULT_HANDSHAKE_TIMEOUT_MS = 20_000;
/** 普通请求（`tools/list`/`tools/call`）超时。 */
export const DEFAULT_CALL_TIMEOUT_MS = 30_000;

export interface McpClientOptions {
  url: string;
  headers?: Record<string, string>;
  clientName?: string;
  clientVersion?: string;
  handshakeTimeoutMs?: number;
  fetchImpl?: typeof fetch;
}

/** 一个已经握手成功的 MCP server 连接。 */
export class McpClient {
  private readonly transport: HttpTransport;
  /** server 在 `initialize` 响应里回的协议版本（协商结果，见模块注释）。 */
  readonly protocolVersion: string;
  /** server 自报的名字（`serverInfo.name`），可能没有。 */
  readonly serverName: string | null;
  /** server 声明的能力，原样搬。 */
  readonly capabilities: unknown;

  private constructor(transport: HttpTransport, protocolVersion: string, serverName: string | null, capabilities: unknown) {
    this.transport = transport;
    this.protocolVersion = protocolVersion;
    this.serverName = serverName;
    this.capabilities = capabilities;
  }

  /** 走完握手（`initialize` → `notifications/initialized`）。任何一步失败都
   * 干净地抛出去——半握手的连接不会被返回，调用方（`connect.ts`）把它标成
   * unavailable 就完事，没有需要收尸的资源（HTTP 是无状态的，这一点比 stdio
   * 省心）。 */
  static async connect(options: McpClientOptions): Promise<McpClient> {
    const transport = new HttpTransport({ url: options.url, headers: options.headers, fetchImpl: options.fetchImpl });
    const timeout = options.handshakeTimeoutMs ?? DEFAULT_HANDSHAKE_TIMEOUT_MS;

    const params = initializeParams(options.clientName ?? "agent-web", options.clientVersion ?? "0.0.0");
    const result = await transport.request("initialize", params, timeout);
    const parsed = parseInitializeResult(result);

    // 协商结果记下来（不断言），并从下一个请求起随头发出去。
    transport.protocolVersion = parsed.protocolVersion;
    await transport.notify("notifications/initialized", undefined, timeout);

    return new McpClient(transport, parsed.protocolVersion, parsed.serverName, parsed.capabilities);
  }

  /** client 这次提议的版本——留给诊断用：跟 [`protocolVersion`] 对比就知道
   * server 有没有改口，但**两者不同不是错误**。 */
  static get proposedProtocolVersion(): string {
    return CLIENT_PROTOCOL_VERSION;
  }

  /** `tools/list`。顺序原样保留，翻译成注入声明是 `translate.ts` 的事——
   * 这一层不认识 `web:mcp-` 这个前缀。 */
  async listTools(timeoutMs = DEFAULT_CALL_TIMEOUT_MS): Promise<McpTool[]> {
    return parseToolsList(await this.transport.request("tools/list", {}, timeoutMs));
  }

  /** `tools/call`。`toolName` 是**裸名字**（不带 `web:mcp-<server>/` 前缀）。
   * 返回未拍平的 result——拍平成「文本 + isError」是 `tool_result.ts` 的事。 */
  async call(toolName: string, args: unknown, timeoutMs = DEFAULT_CALL_TIMEOUT_MS): Promise<unknown> {
    return await this.transport.request("tools/call", toolsCallParams(toolName, args), timeoutMs);
  }

  /** 显式结束会话（尽力而为，不抛）。 */
  async close(timeoutMs = DEFAULT_CALL_TIMEOUT_MS): Promise<void> {
    await this.transport.terminate(timeoutMs);
  }
}

/** 握手/调用失败时统一的一句话——`connect.ts` 拿它填 `unavailable` 的
 * `reason`，UI 和 console 看到的是同一句。 */
export function describeClientFailure(error: unknown): string {
  if (error instanceof McpTransportError) return error.message;
  if (error instanceof Error) return `${error.name}: ${error.message}`;
  return String(error);
}
