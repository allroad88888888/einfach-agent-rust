// 唯一职责：把 `tools/call` 的 result 拍平成一条工具结果——可见文本 + 是否
// 出错。逐条对应 `crates/agent-mcp/src/tool_result.rs`。
//
// 这是接缝上的落点：066 拿到的就是这两个字段，直接组
// `POST /sessions/{id}/tool_result` 的 `result: { content, is_error }`。
// **MCP 的 wire 形状（`content` 块数组、`isError`）到此为止**——066 那边
// grep 不到任何 MCP 结构，跟服务端 `agent-runtime` 只接「一段文本 + 一个
// 布尔」是同一条边界。

/** 一次 `tools/call` 的结果拍平后的样子。 */
export interface ToolCallOutput {
  text: string;
  isError: boolean;
}

/** 拍平一个 `tools/call` result。
 *
 * - `isError` 缺省 `false`（协议默认成功）。
 * - `content` 逐块取 `text` 拼接。
 * - **没有 text 块**（image/resource 等本模块不翻译的块）时不喂空串——原样
 *   搬 `content` 的 JSON，保守不丢信息。
 * - `content` 整个缺失（不合规）则搬整个 result。 */
export function flattenToolResult(result: unknown): ToolCallOutput {
  const obj = isObject(result) ? result : null;
  const isError = obj !== null && obj.isError === true;
  const content = obj !== null && Array.isArray(obj.content) ? obj.content : null;

  if (content === null) return { text: stringify(result), isError };

  const texts: string[] = [];
  for (const block of content) {
    if (isObject(block) && typeof block.text === "string") texts.push(block.text);
  }
  return { text: texts.length > 0 ? texts.join("\n") : stringify(content), isError };
}

function stringify(value: unknown): string {
  try {
    return JSON.stringify(value) ?? String(value);
  } catch {
    // 循环引用之类——不该出现在 `JSON.parse` 出来的值上，但真出现了也不能
    // 让拍平这一步抛，那会把「工具有结果」变成「工具挂了」。
    return String(value);
  }
}

function isObject(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}
