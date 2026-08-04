// 唯一职责：**本前端的能力声明源**——「此刻我愿意报给 server 的工具/skill 是
// 哪些、谁来执行它们」这一份清单。线上形状从 `@agent/protocol` 拿（061 生成的
// `Capabilities`/`CapabilityTool`），具体工具在 `./demo-tools.ts`，这里只做
// 登记、汇总、按名字查实现。
//
// 类型来源：**用生成的类型，没有手写镜像**。065 动手时 061 还没落地（Rust 侧
// 没有 `Capabilities`，也就没有东西可生成），一度在本目录放过一份本地最小接口；
// 061 把 `Capabilities` 挂上 ts-rs 导出、`packages/protocol/src/generated/
// Capabilit*.ts` 出现之后立刻换掉了——决策 2 的规矩是协议类型只从生成物来，
// 上行请求体也不例外（`packages/protocol/src/index.ts` 那段 061 的注释写明了
// 这是目前唯一从那个入口出去的请求体类型）。
//
// 时机：声明**只在建会话那一次**发出去（接缝 §三：建会话时一次性声明，不做
// 运行时增删；§八点 3：`existing`/`recovered` 时传了也不生效）。所以要注入的
// 东西必须在 `createSession` **之前**登记完——`registerWebTool` 不是一个「随时
// 都能加能力」的口子，晚了就得等下一个会话。
//
// 065 只做「把声明发出去」这一半。留给下游的两个接线点：
// - **066（执行）**：拿 `tool_executing` 帧的 `request.tool` 调
//   `findWebTool(name)`。**返回 `undefined` 要回传 `is_error`，不要静默**——
//   060 之后不回传只会等到超时，回传错误能让模型当场自纠。
// - **067（MCP 客户端）**：`tools/list` 翻译完，逐个 `registerWebTool(tool,
//   impl)`（`impl` 就是转发一次 `tools/call`），然后 `main.ts` 再
//   `createSession(webCapabilities())`。067 只需要改 `main.ts` 的调用顺序 +
//   新建它自己的 `src/mcp/`，本目录两个文件一行都不用动。
import type { Capabilities, CapabilitySkill, CapabilityTool } from "@agent/protocol";

import { demoToolImpls, demoTools } from "./demo-tools";

/** 前端注入的工具名前缀（接缝 §七细节 1：MCP 也用 `web:`，不用 `mcp:`——那个
 * 前缀被 `location_of` 判成 `Location::Server`，是部署方配的服务端 MCP）。 */
export const WEB_TOOL_PREFIX = "web:";

/** 名字合规吗。**这不是替 server 做校验**（061 的 `http::capabilities::validate`
 * 才是权威，不合规一律 400），是给调用方一个在发请求之前自己拦住的机会：一个
 * 坏名字会让整个 `POST /sessions` 400、会话根本建不起来——067 的失败隔离条款
 * （一个 MCP server 有问题不能拖垮会话）要求它翻译完自己先过这一关。 */
export function isWebToolName(name: string): boolean {
  return name.startsWith(WEB_TOOL_PREFIX) && name.length > WEB_TOOL_PREFIX.length;
}

/** 一个 `web:` 工具在前端这边的实现。`input` 是模型给的参数（下行
 * `ToolCallRequest.input`，`JsonValue`），返回值是要塞进
 * `POST /tool_result` 的 `content`；抛异常 = 这次调用失败（066 负责把它翻成
 * `is_error`）。 */
export type WebToolImpl = (input: unknown) => string | Promise<string>;

const declaredTools: CapabilityTool[] = [];
const implementations = new Map<string, WebToolImpl>();

/** 065 不声明任何 skill——服务端收 `capabilities.skills` 那半是 062 的事。
 * 数组先摆在这里，`webCapabilities()` 已经会带上它（非空才带），所以真要声明
 * skill 时只用往这里 `push`。 */
const declaredSkills: CapabilitySkill[] = [];

/** 登记一个工具：声明 + 实现一起进来，**不允许只声明不实现**——那等于告诉
 * 模型「我有这个能力」，然后在它真调用时给一个错误。
 *
 * 名字不合规直接抛（`isWebToolName`）：坏名字会让整个 `POST /sessions` 被 061
 * 400 掉、会话建不起来，与其到那时候排查，不如在登记这一刻就炸在调用方脸上。 */
export function registerWebTool(tool: CapabilityTool, impl: WebToolImpl): void {
  if (!isWebToolName(tool.name)) {
    throw new Error(`能力名字必须带 "web:" 前缀（061 会拒绝别的前缀，不会帮你改写）：${tool.name}`);
  }
  if (implementations.has(tool.name)) {
    throw new Error(`能力 ${tool.name} 重复登记——两份声明撞成一个名字，模型分不清调的是哪个`);
  }
  declaredTools.push(tool);
  implementations.set(tool.name, impl);
}

for (const tool of demoTools) {
  const impl = demoToolImpls[tool.name];
  if (!impl) throw new Error(`demoTools 声明了 ${tool.name}，但 demoToolImpls 里没有对应实现`);
  registerWebTool(tool, impl);
}

/** 这次建会话要发出去的声明。**没有任何能力时返回 `undefined`**，让
 * `createSession` 落回「不带 `capabilities` 字段」那条老路——请求体逐字节跟
 * 065 之前一样（接缝 §四的向后兼容不是靠 server 宽容，是这边真的不发）。
 *
 * 数组顺序无所谓：server 侧按名字排序后才进 prompt（红线 11，062 装配的职责），
 * 这边不需要也不应该假设自己的顺序会被保留。 */
export function webCapabilities(): Capabilities | undefined {
  if (declaredTools.length === 0 && declaredSkills.length === 0) return undefined;
  const capabilities: Capabilities = {};
  if (declaredTools.length > 0) capabilities.tools = [...declaredTools];
  if (declaredSkills.length > 0) capabilities.skills = [...declaredSkills];
  return capabilities;
}

/** 按工具名找实现（066 的入口）。找不到返回 `undefined`——调用方**必须**把它
 * 翻成一次 `is_error` 回传，别沉默。 */
export function findWebTool(name: string): WebToolImpl | undefined {
  return implementations.get(name);
}
