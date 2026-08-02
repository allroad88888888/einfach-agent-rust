// 唯一职责：`turn_guard` 帧的一行小字（issue 033 验收原文「GuardReport 一行
// 小字」）——usage + 三层判读 + adjustments 摘成一行,不摊开 `GuardReport`
// 每个字段的完整结构（那是诊断工具该做的事,不是 M3 最小客户端）。
import type { Adjustment, AgentId, GuardReport, TokenUsage } from "@agent/protocol";

import { appendToTimeline, el } from "../dom";

export function turnGuard(usage: TokenUsage, report: GuardReport, adjustments: Adjustment[], agent: AgentId): void {
  const cached = usage.cached === null ? "—" : String(usage.cached);
  const parts = [
    `usage prompt=${usage.prompt} completion=${usage.completion} cached=${cached}`,
    `drift=${tag(report.drift)}`,
    `reconcile=${tag(report.reconcile)}`,
    `window=${tag(report.window)}`,
  ];
  if (adjustments.length > 0) {
    parts.push(`adjustments=${adjustments.map(tag).join(",")}`);
  }
  appendToTimeline(el("div", "guard-line", parts.join(" · ")), agent);
}

/** 判别联合（外部标签："Tag" 或 `{ Tag: {...} }`）→ 给人看的短字符串。只取
 * 标签名 + 紧凑负载,不是完整展开——"一行小字" 的分寸。 */
function tag(value: unknown): string {
  if (typeof value === "string") return value;
  if (value && typeof value === "object") {
    const entry = Object.entries(value)[0];
    if (!entry) return "?";
    const [name, payload] = entry;
    const compact = payload && typeof payload === "object" && Object.keys(payload).length > 0 ? JSON.stringify(payload) : "";
    return `${name}${compact}`;
  }
  return String(value);
}
