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
export type { PollFrame } from "./generated/PollFrame";
export type { PollResponse } from "./generated/PollResponse";
// 072：`GET /sessions/{id}/pending_tools` 的响应体——此刻还欠着宿主回传的远端
// 调用。宿主执行一次 `web:` 工具之前拿它求证（帧只是触发器，服务端的等待槽才是
// 判据），每次连上再拉一次把欠的活补掉。`Frame`/`SessionEvent` 一个字节没动。
export type { PendingTool } from "./generated/PendingTool";
export type { PendingToolsResponse } from "./generated/PendingToolsResponse";
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
// 054：轮末孤儿告警（`SessionEvent::orphaned_child`）的 `fate` 载荷——
// `render/notice.ts` 直接按它的三个变体组措辞，同上一条注释的道理收拢一份。
export type { OrphanFate } from "./generated/OrphanFate";
// 211：自驱动的一轮为什么没自己开（`SessionEvent::auto_turn_held` 的 `reason`
// 载荷）——`render/notice.ts` 按它的三个变体组措辞，同上一条注释的道理。
export type { AutoTurnHold } from "./generated/AutoTurnHold";

// 049：web 活树面板（`render/agent_tree.ts`）直接点名了 `AgentTree` 本身
// （`GET /sessions/:id/agents` 的响应体、也是 `SessionEvent::agent_tree` 变体
// 的 `data`）以及它内层的 `AgentNode`/`AgentActivity`——同上一条注释的道理，
// 收拢到这个入口，不绕去 `generated/` 内部找。
export type { AgentTree } from "./generated/AgentTree";
export type { AgentNode } from "./generated/AgentNode";
export type { AgentActivity } from "./generated/AgentActivity";

// 061：上行的另一半——`POST /sessions` 请求体里 `capabilities` 那一段（宿主
// 声明自己有哪些 tool/skill，docs/HOST-CAPABILITIES.md §四）。这是目前唯一从
// 这个入口出去的**请求体**类型：下行的 `Frame`/`SessionEvent` 之外，前端拼
// 声明时也该用生成的形状，不手写一份会漂移的镜像（065）。
// `CapabilityReversibility` 是**小写** union（`"pure"|…`），跟下行
// `ToolCallRequest.reversibility` 那个大写的 `Reversibility` 不是同一套拼法——
// 一个是宿主报进来的，一个是 core 落盘/推事件用的，别混。
export type { Capabilities } from "./generated/Capabilities";
export type { CapabilityTool } from "./generated/CapabilityTool";
export type { CapabilitySkill } from "./generated/CapabilitySkill";
export type { CapabilityReversibility } from "./generated/CapabilityReversibility";

// 092：远端宿主工具的强确认协议。执行端必须从这个入口取得 claim/result/status
// 的请求、回执和判别联合，不能镜像 Rust wire 类型；否则重试/状态语义容易漂移。
export type { ToolClaimRequest } from "./generated/ToolClaimRequest";
export type { ToolClaimResponse } from "./generated/ToolClaimResponse";
export type { ToolClaimDisposition } from "./generated/ToolClaimDisposition";
export type { ToolResultV2Request } from "./generated/ToolResultV2Request";
export type { ToolResultResponse } from "./generated/ToolResultResponse";
export type { ToolResultDisposition } from "./generated/ToolResultDisposition";
export type { ToolOutcome } from "./generated/ToolOutcome";
export type { ToolFailure } from "./generated/ToolFailure";
export type { ToolStatusResponse } from "./generated/ToolStatusResponse";
export type { ToolCallState } from "./generated/ToolCallState";
export type { ToolTerminalStatus } from "./generated/ToolTerminalStatus";
export type { ToolTerminalOrigin } from "./generated/ToolTerminalOrigin";

// 109：压缩可见性（`render/compaction.ts`）直接点名了这几个类型——
// `SessionEvent::compaction_applied`/`tool_results_cleared` 帧的 `summary_id`
// 字段类型，以及 `GET /sessions/{id}/compaction_record` 的响应体一整套（完整
// 记录 `Message`/`ContentBlock`/`Role`，展开原文走的是这条链，不经 `SendPlan`；
// 摘要库 `SummaryEntry`，正文来自 `Slot::Summaries`）。同上面几条注释的道理，
// 收拢到这个入口，不绕去 `generated/` 内部找。
export type { SummaryId } from "./generated/SummaryId";
export type { Message } from "./generated/Message";
export type { ContentBlock } from "./generated/ContentBlock";
export type { Role } from "./generated/Role";
export type { CompactionRecordResponse } from "./generated/CompactionRecordResponse";
export type { SummaryEntry } from "./generated/SummaryEntry";
