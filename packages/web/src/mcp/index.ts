// 唯一职责：本模块的**导出面**。下游（065 的声明源、066 的执行派发、
// `main.ts` 的接线）只从这里 import，不用知道内部怎么分文件。
//
// 形态、代价、接线点见同目录 `README.md`。

// 主入口：一批配置 → 可注入的工具 + 每个源的可用性 + 按名字路由的 call。
export { connectMcpServers } from "./connect";
export type { ConnectMcpOptions, McpServerConfig, McpToolSource } from "./connect";

// 与 065 的接线（`registerWebTool` 那一根线）。
export { registerMcpTools } from "./register";

// 名字规则：066 判「这次 `tool_executing` 是不是 MCP 工具」用
// `parseInjectedToolName`（或 `McpToolSource.handles`）。
export { INJECTED_TOOL_PREFIX, injectedToolName, isInjectableSegment, parseInjectedToolName, translateTool } from "./translate";

// 每个 server 的可用性——UI 渲染「这个源不可用」用。
export { describeStatus, isConnected } from "./status";
export type { McpAvailability, McpServerStatus } from "./status";

// 结果拍平：`tools/call` 的 result → 「文本 + isError」，066 直接组
// `POST /tool_result` 的 body。
export { flattenToolResult } from "./tool_result";
export type { ToolCallOutput } from "./tool_result";

// 下面这些是「想自己拿一个连接用」时才需要的低层件，正常接线用不到。
export { CLIENT_PROTOCOL_VERSION } from "./protocol";
export { DEFAULT_CALL_TIMEOUT_MS, DEFAULT_HANDSHAKE_TIMEOUT_MS, McpClient } from "./client";
export type { McpClientOptions } from "./client";
export type { Annotations, InitializeResult, McpTool } from "./protocol";
export { McpProtocolError, McpRpcError, McpTransportError } from "./errors";
export type { ProtocolErrorKind } from "./errors";
