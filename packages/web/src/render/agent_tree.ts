// 唯一职责：把一份 `AgentTree` 快照整棵重画进树面板 DOM——**哑渲染器**
// （`docs/OBSERVABILITY.md` §「snapshot，不是 reconstruct」）：不维护自己的
// agent 状态机，不从零散事件推断父子关系，每次收到新快照就清空面板重建。跟
// CLI `/agents`（047 `agent-cli/src/print/agent_tree.rs`）共用同一份
// `agent_tree()` 数据——这里只是渲成 DOM 树而不是一段缩进文本，格式化规则
// （短 id、activity 呈现、task 折叠+截断）照抄那份文本渲染器的判据，两个壳的
// 树不该在任何呈现细节上分叉。
import type { AgentActivity, AgentNode, AgentTree } from "@agent/protocol";

import { agentTreeEl, shortAgentLabel } from "../dom";

/** task 文本超过这个字符数就截断——跟 `agent_tree.rs` 的
 * `TASK_DISPLAY_MAX_CHARS` 同一个数字，纯展示用的行宽控制。 */
const TASK_DISPLAY_MAX_CHARS = 60;

/** 整棵重画：清空容器、按 `nodes`（`live_agents()` 保证的字典序，父先于子，
 * 见 `AgentTree` 类型文档「树渲染不该抖」）逐个起一行。不做 DOM diff——树很小
 * （深 ≤ 3 / 子 ≤ 8，048 issue 记录的量级），重画的成本可以忽略，换来的是
 * 「不会因为某条增量 patch 逻辑漏一处而跟 core 的树分叉」。 */
export function renderAgentTree(tree: AgentTree): void {
  agentTreeEl.replaceChildren(...tree.nodes.map(renderRow));
}

function renderRow(node: AgentNode): HTMLElement {
  const row = document.createElement("div");
  row.className = "tree-row";
  row.style.paddingLeft = `${node.depth * 1.1}rem`;

  const dot = document.createElement("span");
  dot.className = `tree-dot tree-dot--${activityDotClass(node.activity)}`;
  dot.title = describeActivity(node.activity);

  const id = document.createElement("span");
  id.className = "tree-id";
  id.textContent = shortAgentLabel(node.id);

  const activity = document.createElement("span");
  activity.className = "tree-activity";
  activity.textContent = describeActivity(node.activity);

  const task = document.createElement("span");
  task.className = "tree-task";
  task.textContent = describeTask(node.task);

  row.append(dot, id, activity, task);
  return row;
}

/** 状态灯的颜色档位——对应 issue 049「状态灯（Idle/Thinking/ToolsPending/
 * 终态）」四档，`Working`/`Failed` 各自的中间态直接照 `AgentActivity` 的判别
 * 标签映射，不用额外维护一张单独的枚举表。 */
function activityDotClass(activity: AgentActivity): string {
  if (activity === "Idle") return "idle";
  if (activity === "Thinking") return "thinking";
  if ("Working" in activity) return "working";
  if ("Done" in activity) return "done";
  return "failed";
}

/** [`AgentActivity`] 的可读呈现——跟 `agent_tree.rs::describe_activity` 逐行
 * 对应：`Idle`/`Thinking`/`Working(工具…)`/`Done`(`Done(truncated)`)/
 * `Failed(原因)`。 */
function describeActivity(activity: AgentActivity): string {
  if (activity === "Idle") return "Idle";
  if (activity === "Thinking") return "Thinking";
  if ("Working" in activity) {
    const { tools } = activity.Working;
    return tools.length === 0 ? "Working" : `Working(${tools.join(", ")})`;
  }
  if ("Done" in activity) {
    return activity.Done.truncated ? "Done(truncated)" : "Done";
  }
  return `Failed(${activity.Failed.reason})`;
}

/** task 文本：折叠内部空白（含换行），再按字符数截断——跟
 * `agent_tree.rs::describe_task` 同一条规则。没有 task 就是占位符，跟
 * `AgentNode.task` 文档的原则一致：`None` 和「写了但恰好是空串」不该长得
 * 一样。 */
function describeTask(task: string | null): string {
  if (task === null) return "(无任务文本)";
  const collapsed = task.split(/\s+/).filter((s) => s.length > 0).join(" ");
  return truncateChars(collapsed, TASK_DISPLAY_MAX_CHARS);
}

function truncateChars(text: string, maxChars: number): string {
  const chars = Array.from(text);
  if (chars.length <= maxChars) return text;
  return `${chars.slice(0, maxChars).join("")}…`;
}
