// 唯一职责：把一份 [`McpToolSource`] 登记进 065 的声明源
// （`src/capabilities/index.ts` 的 `registerWebTool`）。**这是 067 与 065 之间
// 那一根线**，单独一个文件，好让「MCP 客户端」本身不知道 065 长什么样。
//
// 065 的 `index.ts` 头注释已经把接线点写死了：
//
// > **067（MCP 客户端）**：`tools/list` 翻译完，逐个 `registerWebTool(tool,
// > impl)`（`impl` 就是转发一次 `tools/call`），然后 `main.ts` 再
// > `createSession(webCapabilities())`。
//
// 本文件就是那句话的实现。**`main.ts` 的那两行本 issue 不改**（那是 065/066
// 的地盘）——见 `README.md`「接线点」。

import { registerWebTool } from "../capabilities";
import type { McpToolSource } from "./connect";

/** 把 `source.tools` 全部登记进 065 的声明源，返回真正登记成功的个数。
 *
 * 单个工具登记失败（名字不合规、跟已有能力撞名）**只跳过它并告警**——失败
 * 隔离一路贯彻到这一层：一个坏工具不该让整批 MCP 工具都注入不进去。 */
export function registerMcpTools(source: McpToolSource): number {
  let registered = 0;
  for (const tool of source.tools) {
    try {
      registerWebTool(tool, async (input) => {
        const output = await source.call(tool.name, input);
        // 抛异常 = 这次调用失败，066 负责把它翻成 `is_error` 回传。MCP 的
        // `isError: true` 是同一件事的 wire 形状，这里对齐过去。
        if (output.isError) throw new Error(output.text);
        return output.text;
      });
      registered += 1;
    } catch (error) {
      console.warn(`[mcp] 登记 ${tool.name} 失败，跳过：${error instanceof Error ? error.message : String(error)}`);
    }
  }
  return registered;
}
