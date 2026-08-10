// 唯一职责：收到 web 工具调用后，先取得服务端认领，再执行一次并提交终态。
import type { AgentId, Frame, PendingTool, ToolCallId, ToolCallRequest, ToolCallState, ToolOutcome } from "@agent/protocol";

import { fetchPendingTools } from "./api";
import {
  RemoteToolProtocolError,
  claimRemoteTool,
  createRemoteToolClaim,
  createRemoteToolSubmission,
  fetchRemoteToolStatus,
  submitRemoteToolOutcome,
  type RemoteToolClaim,
  type RemoteToolSubmission,
} from "./tool-claim";
import { findWebTool } from "./capabilities";

/** 与 Rust HTTP 的 `MAX_TOOL_RESULT_BODY_BYTES` 对齐，按 UTF-8 字节量。 */
const MAX_RESULT_BYTES = 1024 * 1024;
const TRUNCATION_NOTE = "\n\n[前端截断：完整结果超过 1 MiB 的回传上限，这里只带回了开头这一段。需要更多请缩小查询范围再调一次。]";
const encoder = new TextEncoder();

export interface ToolExecutor {
  (frame: Frame): void;
  sweep(): Promise<void>;
}

interface ExecutionAttempt {
  claim: RemoteToolClaim;
  submission?: RemoteToolSubmission;
  running?: Promise<void>;
  closed: boolean;
}

/**
 * 每个 session 有自己的 attempt 表。它不决定“能不能执行”：真正的 compare-and-set
 * 在 `POST /tool_claim` 的 actor 内；此表只保存同一次本地执行的稳定 id 与待重试回执。
 */
export function createToolExecutor(sessionId: string): ToolExecutor {
  const attempts = new Map<string, ExecutionAttempt>();

  function execute(frame: Frame): void {
    if (frame.event.type !== "tool_executing" || frame.event.data.request.location !== "Web") return;
    schedule({ agent: frame.agent, call_id: frame.event.data.call_id, request: frame.event.data.request });
  }

  async function sweep(): Promise<void> {
    const scheduled: Promise<void>[] = [];
    for (const pending of await fetchPendingTools(sessionId)) {
      if (pending.request.location === "Web") scheduled.push(schedule(pending));
    }
    await Promise.all(scheduled);
  }

  function schedule(pending: PendingTool): Promise<void> {
    const key = `${pending.agent}\u0000${pending.call_id}`;
    let attempt = attempts.get(key);
    if (attempt?.closed) return Promise.resolve();
    if (attempt?.running !== undefined) return attempt.running;
    attempt ??= { claim: createRemoteToolClaim(), closed: false };
    attempts.set(key, attempt);
    const running = drive(sessionId, pending, attempt).finally(() => {
      attempt.running = undefined;
    });
    attempt.running = running;
    return running;
  }

  return Object.assign(execute, { sweep });
}

async function drive(sessionId: string, pending: PendingTool, attempt: ExecutionAttempt): Promise<void> {
  try {
    // 结果已产生但上次网络结果不明：只重投同一 submission，绝不重做副作用。
    if (attempt.submission !== undefined) {
      await submit(sessionId, pending.agent, pending.call_id, attempt);
      attempt.closed = true;
      return;
    }

    const grant = await claimRemoteTool(sessionId, pending.agent, pending.call_id, attempt.claim);
    if (grant.disposition === "ignored") {
      attempt.closed = true;
      return;
    }
    // 协议语义：claimed/already_claimed_by_you 一定带 request（Rust 侧
    // `ToolClaimResponse.request` 是 Option，claimed 时必填）。TS 生成类型里
    // `request` 是可选字段、判别联合又收窄不了，这里显式守卫——响应形状异常
    // 时放弃本端执行（不重试：服务端一直缺 request 的话重试也白搭）。
    if (grant.request === undefined) {
      attempt.closed = true;
      console.error(
        `[web-tool] ${pending.request.tool}（${pending.call_id}）认领成功但响应缺 request，本端不执行`,
      );
      return;
    }

    // 使用 claim 响应里的 request，而不是 SSE/待办中的副本；这份是 CAS 同一事务给出的 grant。
    attempt.submission = createRemoteToolSubmission(await produceOutcome(grant.request));
    await submit(sessionId, pending.agent, pending.call_id, attempt);
    attempt.closed = true;
  } catch (error) {
    if (error instanceof RemoteToolProtocolError) {
      await reportProtocolState(sessionId, pending, attempt.claim, error);
      // 明确的协议拒绝不可通过重试解决；网络错误则保留 attempt，下一次 sweep 只会重投同一 id。
      attempt.closed = true;
      return;
    }
    const phase = attempt.submission === undefined ? "认领" : "回执";
    console.error(`[web-tool] ${pending.request.tool}（${pending.call_id}）${phase}的网络结果未知：${describe(error)}；下次 sweep 将复用同一凭据重试，绝不重复执行工具`);
  }
}

async function submit(sessionId: string, agent: AgentId, callId: ToolCallId, attempt: ExecutionAttempt): Promise<void> {
  const result = await submitRemoteToolOutcome(sessionId, agent, callId, attempt.claim, attempt.submission!);
  console.info(`[web-tool] ${callId} 已${result.disposition === "duplicate" ? "确认重复回执" : "提交"}为 ${result.terminal_status}`);
}

async function reportProtocolState(
  sessionId: string,
  pending: PendingTool,
  claim: RemoteToolClaim,
  error: RemoteToolProtocolError,
): Promise<void> {
  if (error.code === "tool_call_unknown" || error.code === "status_not_retained") {
    console.info(`[web-tool] ${pending.request.tool}（${pending.call_id}）${error.code}，本端不执行`);
    return;
  }
  if (error.code !== "tool_call_terminal") {
    console.error(`[web-tool] ${pending.request.tool}（${pending.call_id}）协议拒绝：${error.message}`);
    return;
  }
  try {
    const status = await fetchRemoteToolStatus(sessionId, pending.agent, pending.call_id, claim);
    console.info(`[web-tool] ${pending.request.tool}（${pending.call_id}）已结束：${describeTerminalState(status.state)}`);
  } catch (statusError) {
    console.info(`[web-tool] ${pending.request.tool}（${pending.call_id}）已结束（状态详情暂不可读）：${describe(statusError)}`);
  }
}

function describeTerminalState(state: ToolCallState): string {
  switch (state) {
    case "failed": return "执行失败（failed）";
    case "cancelled": return "调用已取消（cancelled）";
    case "outcome_unknown": return "已认领但执行结果未知（outcome_unknown），禁止自动重跑";
    case "unclaimed_timeout": return "无人认领而超时（unclaimed_timeout）";
    default: return state;
  }
}

async function produceOutcome(request: ToolCallRequest): Promise<ToolOutcome> {
  const implementation = findWebTool(request.tool);
  if (implementation === undefined) {
    return failed("tool_not_found", `本前端没有实现工具 ${request.tool}。请改用已声明的工具，或者换个办法完成这件事。`, false);
  }
  try {
    return { status: "succeeded", content: fitToLimit(await implementation(request.input)) };
  } catch (error) {
    const message = `工具 ${request.tool} 执行失败：${describe(error)}`;
    return failed("tool_execution_failed", fitToLimit(message), false);
  }
}

function failed(code: string, message: string, retryable: boolean): ToolOutcome {
  return { status: "failed", error: { code, message, retryable } };
}

/** 按 UTF-8 字节裁到上限，并确保 UTF-8 多字节字符不会被从中间切开。 */
export function fitToLimit(content: string): string {
  const bytes = encoder.encode(content);
  if (bytes.length <= MAX_RESULT_BYTES) return content;
  let keep = Math.max(MAX_RESULT_BYTES - encoder.encode(TRUNCATION_NOTE).length, 0);
  while (keep > 0 && (bytes[keep] & 0xc0) === 0x80) keep -= 1;
  return new TextDecoder().decode(bytes.subarray(0, keep)) + TRUNCATION_NOTE;
}

function describe(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}
