// 唯一职责：时间线上的压缩可见性（issue 109）——两类标记（生成了摘要 / 清除了
// 工具结果）在发生的那一刻挂上时间线，点开能看到原文，且随 `/undo` `/redo`
// 精确地跟着隐藏/恢复。
//
// 跟 `render/user_input.ts`（090 的先例）同一个模式：本地按 `turn_id` 记一份
// 「这一轮挂了哪些标记」，`undo applied`/`redo applied` 帧带的 `turn_id` 直接
// 决定要不要隐藏/恢复——不是「一次 undo 弹一个」的朴素栈（那个假设在压缩由异步
// 子 agent 产出、可能跨轮落地时不成立，见 `agent_runtime::RunnerEvent::
// CompactionApplied` 的字段文档），而是精确匹配 `turn_id`：同一轮最多产出一条
// 标记（`Ladder::fire_once` 一轮只判一次），但同一个 `turn_id` 可能有 0 条。
//
// 展开原文时才懒加载 `GET /sessions/:id/compaction_record`（109 接线约束 1：
// 走完整记录，不经 `SendPlan`），同一条连接内只请求一次、结果缓存复用。
import type { AgentId, CompactionRecordResponse } from "@agent/protocol";

import { appendToTimeline, el } from "../dom";
import { fetchCompactionRecord } from "../api";
import { describeMessage, findToolResultContent } from "./compaction_text";

interface MarkerEntry {
  turnId: number;
  agent: AgentId;
  build(): HTMLElement;
  node?: HTMLElement;
}

export interface CompactionTimeline {
  applied(turnId: number, upto: number, summaryId: string, agent: AgentId): void;
  cleared(turnId: number, callIds: readonly string[], agent: AgentId): void;
  undo(turnId: number): void;
  redo(turnId: number): void;
}

export function createCompactionTimeline(sessionId: string): CompactionTimeline {
  const entries: MarkerEntry[] = [];
  let cached: Promise<CompactionRecordResponse> | undefined;
  const record = (): Promise<CompactionRecordResponse> => (cached ??= fetchCompactionRecord(sessionId));

  function mount(entry: MarkerEntry): void {
    entry.node = entry.build();
    appendToTimeline(entry.node, entry.agent);
  }

  return {
    applied(turnId, upto, summaryId, agent) {
      const entry: MarkerEntry = { turnId, agent, build: () => buildSummaryMarker(turnId, upto, summaryId, record) };
      entries.push(entry);
      mount(entry);
    },
    cleared(turnId, callIds, agent) {
      const entry: MarkerEntry = { turnId, agent, build: () => buildClearedMarker(turnId, callIds, record) };
      entries.push(entry);
      mount(entry);
    },
    undo(turnId) {
      for (const entry of entries) {
        if (entry.turnId === turnId && entry.node) {
          entry.node.remove();
          entry.node = undefined;
        }
      }
    },
    redo(turnId) {
      for (const entry of entries) {
        if (entry.turnId === turnId && !entry.node) mount(entry);
      }
    },
  };
}

/** 一张标记卡：标题 + 「展开原文」按钮，点开才 `record()` 一次（惰性、按需，
 * 不随事件预取——一条会话可能压缩很多次，没人点开的就不必下载完整记录）。 */
function buildMarker(
  title: string,
  renderDetails: (container: HTMLElement, data: CompactionRecordResponse) => void,
  record: () => Promise<CompactionRecordResponse>,
): HTMLElement {
  const card = el("div", "compaction-marker");
  card.append(el("div", "compaction-marker-title", title));
  const toggle = el("button", "compaction-toggle", "展开原文");
  const details = el("div", "compaction-details");
  details.hidden = true;
  let loaded = false;
  toggle.addEventListener("click", () => {
    if (details.hidden && !loaded) {
      toggle.textContent = "加载中…";
      void record()
        .then((data) => {
          loaded = true;
          renderDetails(details, data);
          details.hidden = false;
          toggle.textContent = "收起原文";
        })
        .catch((e: unknown) => {
          details.hidden = false;
          toggle.textContent = "展开原文";
          const msg = e instanceof Error ? e.message : String(e);
          details.replaceChildren(el("div", "error-line", `加载失败：${msg}`));
        });
      return;
    }
    details.hidden = !details.hidden;
    toggle.textContent = details.hidden ? "展开原文" : "收起原文";
  });
  card.append(toggle, details);
  return card;
}

function buildSummaryMarker(
  turnId: number,
  upto: number,
  summaryId: string,
  record: () => Promise<CompactionRecordResponse>,
): HTMLElement {
  const title = `🗜 压缩：生成了一份摘要，覆盖前 ${upto} 条消息（turn=${turnId}）`;
  return buildMarker(title, (container, data) => renderSummaryDetails(container, data, upto, summaryId), record);
}

function buildClearedMarker(
  turnId: number,
  callIds: readonly string[],
  record: () => Promise<CompactionRecordResponse>,
): HTMLElement {
  const title = `🗑 已清除 ${callIds.length} 个工具结果（turn=${turnId}）：${callIds.join(", ")}`;
  return buildMarker(title, (container, data) => renderClearedDetails(container, data, callIds), record);
}

/** 摘要正文从 `summaries`（`Slot::Summaries` 的原样翻译）里按 id 找——109 接线
 * 约束 5。找不到（比如摘要早被更早的一次 undo 撤销）就说明白，不是空白一片。 */
function renderSummaryDetails(container: HTMLElement, data: CompactionRecordResponse, upto: number, summaryId: string): void {
  const summary = data.summaries.find((s) => s.id === summaryId);
  const summaryBlock = el("div", "compaction-summary-text");
  summaryBlock.append(el("h4", undefined, "摘要正文"));
  summaryBlock.append(el("p", undefined, summary?.text ?? "(未找到——可能已经被更早的 undo 撤销)"));

  const original = el("div", "compaction-original");
  original.append(el("h4", undefined, `被盖住的原始轮次（前 ${upto} 条，来自完整记录，不是投影后的样子）`));
  const list = el("ul");
  for (const message of data.messages.slice(0, upto)) {
    list.append(el("li", undefined, describeMessage(message)));
  }
  original.append(list);
  container.replaceChildren(summaryBlock, original);
}

/** 每个被清的调用各去完整记录里找一遍原始 `ToolResult`——`CLEARED_TOOL_RESULT`
 * 占位文本只在投影（发给模型那一份）里出现，这里看到的是原文。 */
function renderClearedDetails(container: HTMLElement, data: CompactionRecordResponse, callIds: readonly string[]): void {
  const list = el("ul");
  for (const id of callIds) {
    const found = findToolResultContent(data.messages, id);
    const line = found
      ? `${id}：${found.isError ? "✗" : "✓"} ${found.content}`
      : `${id}：原文未找到（可能已经被更早的 undo 撤销）`;
    list.append(el("li", undefined, line));
  }
  container.replaceChildren(list);
}
