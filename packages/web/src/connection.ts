// 唯一职责：一条 SSE 连接——开 `EventSource`、把每一帧从 `event.data`
// `JSON.parse` 成 `Frame`（034 起的 agent 归属信封；判别联合本身的收窄发生
// 在调用方对 `frame.event.type` 的 `switch` 里,这个文件不拆包内容）、按
// `FrameWatermark` 去重、把连接状态（连接中/已连接/断线重连中）报给调用方。
// **不手写协议判别联合的声明**——`Frame`/`SessionEvent` 全部从
// `@agent/protocol` 导入（issue 033 决策 2）。
//
// 断线重连本身完全是浏览器原生 `EventSource` 的行为（自动重连、自带
// `Last-Event-ID`）,这里不实现任何重试逻辑——那是决策 31（SSE 补发协议）
// 特意换来的效果,前端只需要不重复渲染（`FrameWatermark`）。
import type { Frame } from "@agent/protocol";

import { eventsUrl } from "./api";
import { FrameWatermark } from "./dedupe";

export type ConnectionState = "connecting" | "open" | "error";

export function connect(sessionId: string, onFrame: (frame: Frame) => void, onStatus: (state: ConnectionState) => void): EventSource {
  const source = new EventSource(eventsUrl(sessionId));
  const watermark = new FrameWatermark();

  onStatus("connecting");
  source.onopen = () => onStatus("open");
  // `EventSource` 断线会自己重连,这里的 "error" 只是报告状态给状态栏看,
  // 不需要手动 `close()` + 重新 `new EventSource()`。
  source.onerror = () => onStatus("error");
  source.onmessage = (ev: MessageEvent<string>) => {
    if (!watermark.admit(ev.lastEventId)) return;
    onFrame(JSON.parse(ev.data) as Frame);
  };

  return source;
}
