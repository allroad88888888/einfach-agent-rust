// 唯一职责：一次装载之后**每个 MCP server 的可用性**——谁连上了、谁没有、
// 为什么。只是数据形状 + 一行人话描述，不做 IO（连接是 `connect.ts` 的事）。
// 对应 `crates/agent-mcp/src/status.rs`。
//
// 失败隔离把「一个 server 起不来」变成一条**结构化状态**，而不是抛异常、
// 也不是吞掉：`connect.ts` 保证这份状态一定有它那一条，UI 拿去渲染、
// console 拿去告警——**别静默**是 067 验收里明写的一条。
//
// 跟 Rust 侧三态的差别：那边有第三档 `Unsupported`（「配置解析成功但 M6 不
// 装载：远端 http/sse 传输未实现」）。浏览器这边远端**就是唯一形态**——
// 没有子进程，stdio 在这个 host 上根本表达不出来（`McpServerConfig` 只有
// `url`，没有 `command`，这不是遗漏，是形态 B 的定义），所以那一档在这里
// 没有对应物，二态穷尽。

/** 一个 server 装载后的可用性。二态穷尽：连上了 / 试了连不上。 */
export type McpAvailability =
  /** 握手 + `tools/list` 成功，工具已经翻进注入声明。带这个 server 贡献了几个。 */
  | { kind: "connected"; toolCount: number }
  /** 尝试连接但失败：连不上 / 握手失败 / 超时 / `tools/list` 失败 / 配置本身
   * 有问题（如 server id 重复）——带原因。**其余 server 照常，会话照常起。** */
  | { kind: "unavailable"; reason: string };

/** 一个 server 的 id + 它装载后的可用性。`connect.ts` 每个 server 产出一条，
 * 顺序 = 配置顺序。 */
export interface McpServerStatus {
  id: string;
  availability: McpAvailability;
}

export function connectedStatus(id: string, toolCount: number): McpServerStatus {
  return { id, availability: { kind: "connected", toolCount } };
}

export function unavailableStatus(id: string, reason: string): McpServerStatus {
  return { id, availability: { kind: "unavailable", reason } };
}

export function isConnected(status: McpServerStatus): boolean {
  return status.availability.kind === "connected";
}

/** 一行人话——UI 状态栏和 console 告警共用同一句，两处不该各编各的措辞。 */
export function describeStatus(status: McpServerStatus): string {
  const availability = status.availability;
  return availability.kind === "connected"
    ? `MCP ${status.id}：已连接，注入 ${availability.toolCount} 个工具`
    : `MCP ${status.id}：不可用（${availability.reason}）——该源的工具未注入，其余照常`;
}
