// 唯一职责：把 `GET /sessions/:id/compaction_record` 拿到的原始 `Message`/
// `ContentBlock` 翻成人能读的一段文字。不碰 DOM、不持状态——`render/compaction.ts`
// 展开一条压缩标记时调用这里，纯格式化。
//
// 109 接线约束 1：这里格式化的是**完整记录**里的原始块（`ContentBlock::
// ToolResult.content` 之类），不是 `SendPlan` 投影之后的样子——展开摘要盖住的
// 原始轮次、被清工具调用的原文，都不会看到 `CLEARED_TOOL_RESULT` 占位或摘要
// 替身，因为它们压根没走 `project()` 这条路。
import type { ContentBlock, Message } from "@agent/protocol";

/** 一条消息 → 一行摘要：`[角色] 块1 / 块2 / ...`。 */
export function describeMessage(message: Message): string {
  const role = message.role === "User" ? "用户" : "助手";
  const blocks = message.blocks.map(describeBlock).join(" / ");
  return `[${role}] ${blocks || "(空)"}`;
}

/** 单个内容块 → 一段文字。`ContentBlock` 是混合判别联合（跟 `Notice` 同款
 * 编码，`in` 运算符判分支），四个变体各给一种简短呈现。 */
function describeBlock(block: ContentBlock): string {
  if ("Text" in block) return block.Text;
  if ("Thinking" in block) return `(思考) ${block.Thinking}`;
  if ("ToolUse" in block) {
    return `调用 ${block.ToolUse.name}(${JSON.stringify(block.ToolUse.input)})`;
  }
  const { content, is_error } = block.ToolResult;
  return `${is_error ? "✗" : "✓"} 结果：${content}`;
}

/** 在完整记录里按 `call_id` 找一个 `ToolResult` 块的原文——「被清工具结果」
 * 展开时用它，找不到就是 `undefined`（防御性：正常情况下清过的调用必然在
 * 记录里，因为清除只改 `SendPlan`，从不删 `Slot::Messages` 里的块）。 */
export function findToolResultContent(
  messages: readonly Message[],
  callId: string,
): { content: string; isError: boolean } | undefined {
  for (const message of messages) {
    for (const block of message.blocks) {
      if ("ToolResult" in block && block.ToolResult.id === callId) {
        return { content: block.ToolResult.content, isError: block.ToolResult.is_error };
      }
    }
  }
  return undefined;
}
