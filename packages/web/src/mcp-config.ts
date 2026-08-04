// 唯一职责：这个**演示前端**从哪读它要连的 MCP server 列表。
//
// 067 把这件事明确留给了接入方（`docs/issues/067` §七原话：「配置从哪来本模块
// 不管——读配置就得决定配置放哪，那是 068 真机接入时的判断」）。这里是那个判断。
//
// # 为什么是 URL 查询参数
//
// 形态 B 的全部要点就是**浏览器自己连**（接缝 §七：server 完全不碰 MCP 协议、
// 不 spawn 任何东西）。既然连接方是这个浏览器，配置放在这个浏览器自己的地址栏
// 是最短的一条路：换一个 server 不用重新构建、不用改代码，dogfood 因此可复现。
//
// 对比被否掉的两个：
// - **写死在源码里**：换个 server 要重新 `pnpm build`，演示价值几乎为零。
// - **localStorage**：看不见、清不掉、还得再造一个 UI 去管它——给一个最小演示
//   前端加状态管理，不划算。
//
// # 默认必须是「空」
//
// 不带这个参数时**一条都不连**，`webCapabilities()` 因此与 067 之前逐字节相同
// （065 验收「不带声明的旧路径仍可用」）。加功能不许改默认行为。
//
// # 这不是「server 帮你连」，两者的安全面完全不同
//
// 参数里的 URL 是**这个浏览器自己**去 fetch 的，跟被否决的形态 A（前端交配置、
// server 去 spawn / 去连 = RCE + SSRF，接缝 §七）不是一回事：server 一个字节都
// 不知道这个地址。它的能力边界就是这个浏览器本来就有的能力边界。
//
// 即便如此，**这是演示前端的取向，不是给生产宿主的建议**——生产宿主该把 MCP
// server 列表放在它自己的配置里，别从 URL 收（接缝 §九，安全那节暂缓）。

import type { McpServerConfig } from "./mcp";

/** 参数名。`?mcp=<id>=<url>`，多个用逗号分隔。 */
export const MCP_PARAM = "mcp";

/**
 * 从一个查询串里解析 MCP server 列表。
 *
 * 形状 `<id>=<url>`，逗号分隔：`?mcp=stamp=http://127.0.0.1:8931/mcp`。
 * 省掉 `<id>=` 也行，那就用 url 的 host 当 id（`http://127.0.0.1:8931/mcp`
 * → `127-0-0-1-8931`）——**id 会进工具名**（`web:mcp-<id>/<tool>`），
 * 所以非法字符一律换成 `-`，剩下的合法性交给 `connectMcpServers`
 * （它对不合规的 id 是整个 server 跳过，理由见那边）。
 */
export function parseMcpServers(search: string): McpServerConfig[] {
  const raw = new URLSearchParams(search).get(MCP_PARAM);
  if (!raw) return [];
  return raw
    .split(",")
    .map((entry) => entry.trim())
    .filter(Boolean)
    .map(toConfig)
    .filter((c): c is McpServerConfig => c !== undefined);
}

function toConfig(entry: string): McpServerConfig | undefined {
  // 只在**第一个** `=` 上切：URL 里可以有 `=`（查询参数），切多了就切坏了。
  const at = entry.indexOf("=");
  const [id, url] = at === -1 ? [undefined, entry] : [entry.slice(0, at), entry.slice(at + 1)];
  if (!url) return undefined;
  return { id: id || idFromUrl(url), url };
}

function idFromUrl(url: string): string {
  let host: string;
  try {
    host = new URL(url).host;
  } catch {
    host = url;
  }
  return host.replace(/[^A-Za-z0-9_-]/g, "-");
}
