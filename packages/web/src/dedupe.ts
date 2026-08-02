// 唯一职责：SSE 帧按 id 去重,让「断网重连补发」不在界面上重复渲染已经见过
// 的帧（issue 033 验收原文）。

/** 帧 id 是 `crates/agent-server/src/http/hub/ring.rs` 里 `RingState` 分配的
 * 单调 u64,同一个 session 的 hub 存活期内严格递增、不重复。`EventSource`
 * 原生重连自带 `Last-Event-ID`,server 侧精确补发/发 `gap` 帧——补发出来的帧
 * id 只会 ≤ 之前已经见过的最大 id,于是去重只需要记住那个「水位线」,不需要一个
 * 会无限增长的「见过的 id 集合」。 */
export class FrameWatermark {
  private lastRenderedId = -1;

  /** `true` = 这一帧没见过,调用方应该渲染;调用本身就是"标记为已渲染",
   * 不能用同一个 id 反复问。`rawId` 是 `MessageEvent.lastEventId`——理论上
   * hub 发的每一帧都带 id,传 `null`（没有 id）时保守地不去重,照渲染。 */
  admit(rawId: string | null): boolean {
    if (rawId === null || rawId === "") return true;
    const id = Number(rawId);
    if (!Number.isFinite(id) || id <= this.lastRenderedId) return false;
    this.lastRenderedId = id;
    return true;
  }
}
