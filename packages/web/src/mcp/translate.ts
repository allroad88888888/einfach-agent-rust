// 唯一职责：一个 MCP 工具 + server id → 一条注入声明（`CapabilityTool`），
// 以及反过来把注入名字拆回 `(server, tool)`。纯函数，零 IO。
// 对应 `crates/agent-mcp/src/translate.rs`，两条规矩原样搬。
//
// # 命名：`web:mcp-<server>/<tool>`
//
// **location 从 `web:` 前缀白拿**，一行 Rust 都不用改：`location_of` 把
// `web:` 判成 `Location::Web`，模型调用会被路由回前端（066 执行）。
// 不能用 `mcp:` ——那个前缀在服务端被判成 `Location::Server`，是**部署方用
// `.mcp.json` 配的**那批 MCP（M6 已做，server 自己连）。两种 MCP 因此能在同
// 一个会话里共存不打架：`mcp:everything/echo`（server 连的）与
// `web:mcp-figma/get_file`（浏览器连的）。
//
// 中间那段 `mcp-` 是给人看的：一眼看得出这个 `web:` 工具的来源是 MCP 而不是
// 前端自己实现的业务工具（065 那批 `web:demo/...`）。
//
// # 可逆性：`readOnlyHint === true → "pure"`，其余一律 `"irreversible"`
//
// 代价不对称：判错成 pure 的代价是重放副作用（重复发邮件/扣款），判错成
// irreversible 只是多问用户一次。一个**未知来源**的 MCP 工具默认可重放 =
// 把数据事故的开关交给第三方。默认必须落保守边——`readOnlyHint` 为 `false`、
// `annotations` 缺失、`annotations` 在但没有 `readOnlyHint`，三种都是
// `"irreversible"`。

// `CapabilityTool` 用 061 生成的那一份（`@agent/protocol`），**不在本模块另抄
// 一遍**——决策 2：协议类型只从生成物来，手写镜像一定会漂。注意它的
// `reversibility` 是**小写** union（`CapabilityReversibility`），跟下行
// `ToolCallRequest.reversibility` 那个大写的 `Reversibility` 不是同一套拼法。
import type { CapabilityTool } from "@agent/protocol";

import type { McpTool } from "./protocol";

/** 注入名字的固定前缀。 */
export const INJECTED_TOOL_PREFIX = "web:mcp-";

/** 061 对 `web:` 之后那一段钉的字符白名单是 `[A-Za-z0-9_/-]`。这里对
 * **单段**（server id、裸工具名）再收紧一格：不许出现 `/`，否则
 * `web:mcp-a/b/c` 拆不回唯一的 `(server, tool)`。 */
const SEGMENT = /^[A-Za-z0-9_-]+$/;

/** 段是否能安全地进注入名字。**过不了就跳过那个工具，绝不 sanitize**——
 * 悄悄改写会让两个不同声明撞成一个（同 055 的 chatid、061 的名字校验）。
 * 跳过发生在 `connect.ts`，那里会同时告警，不静默。 */
export function isInjectableSegment(segment: string): boolean {
  return SEGMENT.test(segment);
}

/** 拼注入名字。调用方负责先用 [`isInjectableSegment`] 过一遍两段。 */
export function injectedToolName(serverId: string, toolName: string): string {
  return `${INJECTED_TOOL_PREFIX}${serverId}/${toolName}`;
}

/** 把注入名字拆回 `(server id, 裸工具名)`；不是本模块产出的名字返回 `null`
 * ——066 靠它判断「这次 `tool_executing` 该不该交给 MCP 走」。 */
export function parseInjectedToolName(name: string): { serverId: string; toolName: string } | null {
  if (!name.startsWith(INJECTED_TOOL_PREFIX)) return null;
  const rest = name.slice(INJECTED_TOOL_PREFIX.length);
  const slash = rest.indexOf("/");
  if (slash < 0) return null;

  const serverId = rest.slice(0, slash);
  const toolName = rest.slice(slash + 1);
  if (!isInjectableSegment(serverId) || !isInjectableSegment(toolName)) return null;
  return { serverId, toolName };
}

/** 一个 MCP 工具 → 一条注入声明。规则见模块注释。 */
export function translateTool(tool: McpTool, serverId: string): CapabilityTool {
  return {
    name: injectedToolName(serverId, tool.name),
    // MCP 允许工具不带描述；注入声明的 description 不是可选字段，缺省落空
    // 字符串——**不拿工具名顶替**，不替 server 编话。
    description: tool.description ?? "",
    schema: tool.inputSchema,
    reversibility: tool.annotations?.readOnlyHint === true ? "pure" : "irreversible",
  };
}
