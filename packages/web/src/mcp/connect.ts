// 唯一职责：**编排**——一批 MCP server 配置 → 一份可注入的工具声明 + 每个源
// 的可用性 + 一个按名字路由的 `call`。对应 `crates/agent-mcp/src/loader.rs`。
//
// # 失败隔离（本 issue 的核心条款）
//
// 一个 server 连不上 / 握手失败 / 超时 / `tools/list` 形状不对 → 标
// `unavailable`（带原因），**其余照常连、会话照常起**。044 在服务端解决过同
// 一个问题，这次在前端重演，判断一模一样：**不是「一个坏了全崩」**。
//
// 具体到这个函数：`connectMcpServers` **不会 reject**（除非调用方自己传了
// 会抛的 `onStatus`）。所有失败都变成 `servers[]` 里的一条结构化状态 + 一条
// `console.warn`。**不静默**是验收明写的——一个源悄悄没了，模型只会表现为
// 「它突然不会用那个工具了」，那种 bug 最难查。
//
// # 顺序
//
// 合并出的工具按**配置顺序**拼接，server 内按 `tools/list` 顺序——两级都确定。
// 但**不要指望这个顺序进 prompt**：server 侧按名字重排（红线 11 由 062 负责，
// HOST-CAPABILITIES §六 第 2 条明写「不按客户端给的数组顺序」）。这里保持
// 确定只是为了自己好调试。

import type { CapabilityTool } from "@agent/protocol";

import { DEFAULT_CALL_TIMEOUT_MS, DEFAULT_HANDSHAKE_TIMEOUT_MS, McpClient, describeClientFailure } from "./client";
import type { McpServerStatus } from "./status";
import { connectedStatus, describeStatus, isConnected, unavailableStatus } from "./status";
import { flattenToolResult } from "./tool_result";
import type { ToolCallOutput } from "./tool_result";
import { isInjectableSegment, parseInjectedToolName, translateTool } from "./translate";

/** 一个要连的 MCP server。
 *
 * **没有 `command`/`args` 字段，这不是遗漏**：那是被否决的形态 A
 * （前端交配置、server 去 spawn = RCE；远端形态 = SSRF，
 * docs/HOST-CAPABILITIES.md §七）。浏览器只连它自己够得着的 http 端点，
 * server 全程不碰 MCP 协议。 */
export interface McpServerConfig {
  /** 会出现在工具名里：`web:mcp-<id>/<tool>`。字符集见
   * `translate.isInjectableSegment`，不合规**整个 server 跳过**（名字过不了
   * 061 的校验会让整个 `POST /sessions` 400，那才是真的拖垮会话）。 */
  id: string;
  /** MCP Streamable HTTP 端点（浏览器发得出去的地址，跨域要 server 开 CORS）。 */
  url: string;
  /** 附加请求头，比如 `Authorization`。 */
  headers?: Record<string, string>;
  handshakeTimeoutMs?: number;
  callTimeoutMs?: number;
}

export interface ConnectMcpOptions {
  clientName?: string;
  clientVersion?: string;
  /** 注入 `fetch`——只为测试。 */
  fetchImpl?: typeof fetch;
  /** 每个 server 出结果时回调一次（顺序 = 完成顺序）。UI 拿它渲染「哪个源
   * 不可用」。**不传也会 `console.warn`**，不存在静默的情况。 */
  onStatus?: (status: McpServerStatus) => void;
}

/** 一次装载的产物。这就是留给 065/066 的接口面。 */
export interface McpToolSource {
  /** 翻译好的注入声明，直接混进 `capabilities.tools`（065）。 */
  tools: CapabilityTool[];
  /** 每个 server 一条：谁连上了、谁没有、为什么。UI 用。 */
  servers: McpServerStatus[];
  /** 这个名字是不是本模块产出的（066 路由 `tool_executing` 用）。 */
  handles: (name: string) => boolean;
  /** 执行一次注入的工具：剥前缀 → `tools/call` → 拍平成「文本 + isError」。
   *
   * **MCP 侧的失败不抛，落成 `isError: true`** ——066 直接把它塞进
   * `POST /tool_result` 就完事，模型当场看到错误自纠（对齐 066「找不到实现
   * 也要回传 is_error，别沉默」）。只有「这名字根本不归我管」才抛，那是
   * 调用方的路由 bug，不该悄悄变成一次工具失败。 */
  call: (name: string, args: unknown) => Promise<ToolCallOutput>;
  /** 显式结束所有会话（尽力而为，不抛）。 */
  close: () => Promise<void>;
}

/** 连一批 MCP server，产出可注入的工具声明。**不 reject。** */
export async function connectMcpServers(
  configs: McpServerConfig[],
  options: ConnectMcpOptions = {},
): Promise<McpToolSource> {
  const clients = new Map<string, { client: McpClient; callTimeoutMs: number }>();
  const results = await Promise.all(
    dedupe(configs).map(async (entry) => {
      if (entry.kind === "rejected") return { tools: [] as CapabilityTool[], status: entry.status };
      return await loadOne(entry.config, clients, options);
    }),
  );

  const tools: CapabilityTool[] = [];
  const servers: McpServerStatus[] = [];
  for (const result of results) {
    tools.push(...result.tools);
    servers.push(result.status);
    report(result.status, options.onStatus);
  }

  let closed = false;
  return {
    tools,
    servers,
    handles: (name) => {
      const parsed = parseInjectedToolName(name);
      return parsed !== null && clients.has(parsed.serverId);
    },
    call: async (name, args) => {
      const parsed = parseInjectedToolName(name);
      const entry = parsed === null ? undefined : clients.get(parsed.serverId);
      if (parsed === null || entry === undefined) {
        throw new Error(`${name} 不是本模块注入的 MCP 工具——调用方路由错了（先用 handles() 判一次）`);
      }
      if (closed) return { text: `MCP 源已关闭，无法调用 ${name}`, isError: true };
      try {
        return flattenToolResult(await entry.client.call(parsed.toolName, args, entry.callTimeoutMs));
      } catch (error) {
        const reason = describeClientFailure(error);
        console.warn(`[mcp] 调用 ${name} 失败：${reason}`);
        return { text: `MCP 调用失败：${reason}`, isError: true };
      }
    },
    close: async () => {
      closed = true;
      await Promise.all([...clients.values()].map(async (entry) => await entry.client.close()));
      clients.clear();
    },
  };
}

/** 连一个 server：握手 → `tools/list` → 翻译。**任何失败都落成一条状态**，
 * 不往上抛。 */
async function loadOne(
  config: McpServerConfig,
  clients: Map<string, { client: McpClient; callTimeoutMs: number }>,
  options: ConnectMcpOptions,
): Promise<{ tools: CapabilityTool[]; status: McpServerStatus }> {
  try {
    const client = await McpClient.connect({
      url: config.url,
      headers: config.headers,
      clientName: options.clientName,
      clientVersion: options.clientVersion,
      handshakeTimeoutMs: config.handshakeTimeoutMs ?? DEFAULT_HANDSHAKE_TIMEOUT_MS,
      fetchImpl: options.fetchImpl,
    });

    const listed = await client.listTools(config.callTimeoutMs ?? DEFAULT_CALL_TIMEOUT_MS);
    const tools: CapabilityTool[] = [];
    for (const tool of listed) {
      // 名字过不了 061 的白名单 → **跳过这一个工具并告警，绝不 sanitize**：
      // 悄悄改写会让两个不同声明撞成一个（同 055 的 chatid）。跳一个总比
      // 整个 `POST /sessions` 被 400 掉、会话建不起来强。
      if (!isInjectableSegment(tool.name)) {
        console.warn(`[mcp] ${config.id} 的工具 "${tool.name}" 名字含不允许的字符，跳过（不改写）`);
        continue;
      }
      tools.push(translateTool(tool, config.id));
    }

    clients.set(config.id, { client, callTimeoutMs: config.callTimeoutMs ?? DEFAULT_CALL_TIMEOUT_MS });
    return { tools, status: connectedStatus(config.id, tools.length) };
  } catch (error) {
    return { tools: [], status: unavailableStatus(config.id, describeClientFailure(error)) };
  }
}

/** id 去重 + 字符集校验。两个 server 用同一个 id 会产出撞名的工具（061 那边
 * 「重复名字 → 拒绝，不做后来居上」），所以后来的那个直接标不可用，不静默
 * 覆盖。 */
function dedupe(
  configs: McpServerConfig[],
): Array<{ kind: "ok"; config: McpServerConfig } | { kind: "rejected"; status: McpServerStatus }> {
  const seen = new Set<string>();
  return configs.map((config) => {
    if (!isInjectableSegment(config.id)) {
      return { kind: "rejected", status: unavailableStatus(config.id, "server id 含不允许的字符（只允许 A-Za-z0-9_-）") };
    }
    if (seen.has(config.id)) {
      return { kind: "rejected", status: unavailableStatus(config.id, "server id 重复——后来的这个不装载，避免撞名") };
    }
    seen.add(config.id);
    return { kind: "ok", config };
  });
}

/** 每条状态都要有人看得见：不可用走 `console.warn`，连上了走 `console.info`。 */
function report(status: McpServerStatus, onStatus?: (status: McpServerStatus) => void): void {
  const line = `[mcp] ${describeStatus(status)}`;
  if (isConnected(status)) console.info(line);
  else console.warn(line);
  onStatus?.(status);
}
