// 唯一职责：`undo`/`redo` 帧的渲染 + `undo_blocked` 时弹出确认（issue 033
// 验收原文，027 的 `/undo!` 语义搬到点击）。
//
// 034 补上了 033 上报的缺口 3：`UndoOutcome::Blocked` 现在带
// `label`/`tool`/`call_id`（`crates/agent-server/src/event/undo_outcome.rs`
// ——actor 侧现查 `Session::barrier_info`，agent-core 的公共读口，CLI 的
// `/undo!` 也走它）,确认弹层因此能显示「越过的是什么」（027 的原则），不用再
// 甩一个裸的 `barrier_seq` 数字给用户猜。
//
// 199 又加了 `cause`：`label`/`tool`/`call_id` 说的是「停在哪一条」，`cause` 说的是
// 「为什么停」。「这一步没交还原函数」（**没碰**）和「还原函数跑挂了」（**碰了，
// 可能做了一半**）对用户是两件事——他要据此决定强不强制越过，只给前一句就是把
// 后一种情况说成了前一种。
import type { AgentId, UndoOutcome } from "@agent/protocol";

import { appendToTimeline, el } from "../dom";
import { sendUndo } from "../api";

export function renderUndoOutcome(kind: "undo" | "redo", outcome: UndoOutcome, sessionId: string, agent: AgentId): void {
  appendToTimeline(el("div", "undo-line", describe(kind, outcome)), agent);
  // `redo` 没有屏障（`Session::redo_turn` 不会产出 `Blocked`,agent-cli 的
  // `undo.rs` 对这个分支直接 `unreachable!`）——这里只对 `undo` 触发确认弹层,
  // 不是因为协议禁止,是因为按现有语义它不会发生。
  if (kind === "undo" && outcome.type === "blocked") {
    confirmForce(outcome.data, sessionId);
  }
}

function describe(kind: string, outcome: UndoOutcome): string {
  switch (outcome.type) {
    case "applied":
      return `${kind} applied：${outcome.data.entries} 条，turn=${outcome.data.turn_id}`;
    case "blocked":
      return `${kind} blocked：停在「${describeBarrier(outcome.data)}」——${describeCause(outcome.data.cause)}，前面还有 ${outcome.data.entries} 条`;
    case "nothing":
      return `${kind}：无可做`;
  }
}

/** 工具名/call_id 都在（目前恒如此——`barrier` 只会在工具结果那条 entry 上
 * 置真）就报「工具名（call_id=...）」，否则退回 entry 的 label——两条路径都
 * 是「越过的是什么」的人话，不是一个裸的 `barrier_seq` 数字。 */
function describeBarrier(data: Extract<UndoOutcome, { type: "blocked" }>["data"]): string {
  if (data.tool && data.call_id) return `${data.tool}（call_id=${data.call_id}）`;
  return data.label;
}

/** 三种成因 → 三句不同的人话（199 §五）。 */
function describeCause(cause: Extract<UndoOutcome, { type: "blocked" }>["data"]["cause"]): string {
  switch (cause.type) {
    case "no_hook":
      return "它没有提供还原函数，本仓无从代它回退";
    case "hook_failed":
      return `它的还原函数跑了但失败了（${cause.data}），可能只还原了一半`;
    case "hook_lost":
      return "它的还原函数随进程重启消失了（函数是闭包，不跨进程），没人能代它回退";
  }
}

function confirmForce(data: Extract<UndoOutcome, { type: "blocked" }>["data"], sessionId: string): void {
  const ok = window.confirm(`撤销停在了：${describeBarrier(data)}。\n${describeCause(data.cause)}。\n前面还有 ${data.entries} 条待定——强制越过继续撤销？`);
  if (!ok) return;
  void sendUndo(sessionId, "turn", true).catch((e: unknown) => {
    appendToTimeline(el("div", "error-line", `强制撤销失败：${(e as Error).message}`), "root");
  });
}
