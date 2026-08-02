// 手写文件（唯一一个）——`src/generated/` 整个目录都是生成物，见那边每个文件
// 自己的「勿手改」头注释。这里只做一件事：把协议的顶层类型收拢到一个入口，
// 下游（未来 apps/*）`import type { SessionEvent } from "@agent/protocol"`，
// 不用知道 `generated/` 内部的文件划分。

export type { SessionEvent } from "./generated/SessionEvent";
export type { UndoOutcome } from "./generated/UndoOutcome";
export type { Command } from "./generated/Command";
export type { Granularity } from "./generated/Granularity";
// 034：SSE 帧 data 的信封——{ agent, event }。`AgentId` 是它的 `agent` 字段
// 类型（`type AgentId = string`），下游按帧归属分栏/打标签时要用到这个类型名。
export type { Frame } from "./generated/Frame";
export type { AgentId } from "./generated/AgentId";

// 033：`packages/web` 的渲染层按帧分发时，直接点名了这几个嵌套在
// `SessionEvent` 变体里的载荷类型（比如 `case "tool_executing"` 之后单独
// 处理 `request: ToolCallRequest`）——TS 只需要导入自己代码里直接写出来的
// 类型名，但既然 032 定的规矩是「协议类型只从这个入口导入」，被直接点名的
// 类型就得在这里也收拢一份，不能让下游绕过这个入口去 `generated/` 内部找。
export type { Adjustment } from "./generated/Adjustment";
export type { DriftVerdict } from "./generated/DriftVerdict";
export type { GuardReport } from "./generated/GuardReport";
export type { Notice } from "./generated/Notice";
export type { TokenUsage } from "./generated/TokenUsage";
export type { ToolCallId } from "./generated/ToolCallId";
export type { ToolCallRequest } from "./generated/ToolCallRequest";
