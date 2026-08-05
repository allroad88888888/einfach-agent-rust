// 唯一职责：执行端取得远端工具认领并提交终态回执。
import type {
  AgentId,
  ToolCallId,
  ToolClaimResponse,
  ToolOutcome,
  ToolResultResponse,
  ToolStatusResponse,
} from "@agent/protocol";

const JSON_HEADERS = { "Content-Type": "application/json" };
const RETRY_COUNT = 3;

export class RemoteToolProtocolError extends Error {
  readonly status: number;
  readonly code: string;

  constructor(
    status: number,
    code: string,
    message: string,
  ) {
    super(`${code}: ${message}`);
    this.status = status;
    this.code = code;
  }
}

export interface RemoteToolClaim {
  claimId: string;
}

export interface RemoteToolSubmission {
  submissionId: string;
  outcome: ToolOutcome;
}

/** 一次执行生命周期的稳定认领凭据。网络重试必须复用它，不能重新生成。 */
export function createRemoteToolClaim(): RemoteToolClaim {
  return { claimId: crypto.randomUUID() };
}

/** 一次终态提交的稳定幂等键；同一份 outcome 的所有重试均复用它。 */
export function createRemoteToolSubmission(outcome: ToolOutcome): RemoteToolSubmission {
  return { submissionId: crypto.randomUUID(), outcome };
}

export function claimRemoteTool(
  sessionId: string,
  agent: AgentId,
  toolCallId: ToolCallId,
  claim: RemoteToolClaim,
): Promise<ToolClaimResponse> {
  return retryUnknown(() => postJson<ToolClaimResponse>(`/sessions/${encodeURIComponent(sessionId)}/tool_claim`, {
    agent,
    tool_call_id: toolCallId,
    claim_id: claim.claimId,
  }));
}

export function submitRemoteToolOutcome(
  sessionId: string,
  agent: AgentId,
  toolCallId: ToolCallId,
  claim: RemoteToolClaim,
  submission: RemoteToolSubmission,
): Promise<ToolResultResponse> {
  return retryUnknown(() => postJson<ToolResultResponse>(`/sessions/${encodeURIComponent(sessionId)}/tool_result`, {
    agent,
    tool_call_id: toolCallId,
    claim_id: claim.claimId,
    submission_id: submission.submissionId,
    outcome: submission.outcome,
  }));
}

/** 终态拒绝后的只读解释。claim id 只通过 header 传递，避免出现在 query 日志中。 */
export function fetchRemoteToolStatus(
  sessionId: string,
  agent: AgentId,
  toolCallId: ToolCallId,
  claim?: RemoteToolClaim,
): Promise<ToolStatusResponse> {
  const query = new URLSearchParams({ agent, tool_call_id: toolCallId });
  const headers = claim === undefined ? undefined : { "X-Tool-Claim-Id": claim.claimId };
  return getJson<ToolStatusResponse>(`/sessions/${encodeURIComponent(sessionId)}/tool_status?${query}`, headers);
}

async function postJson<T>(path: string, body: unknown): Promise<T> {
  const response = await fetch(path, { method: "POST", headers: JSON_HEADERS, body: JSON.stringify(body) });
  return readJson<T>(response);
}

async function getJson<T>(path: string, headers?: HeadersInit): Promise<T> {
  const response = await fetch(path, { headers });
  return readJson<T>(response);
}

async function readJson<T>(response: Response): Promise<T> {
  const text = await response.text();
  if (!response.ok) throw protocolError(response.status, text);
  return JSON.parse(text) as T;
}

function protocolError(status: number, text: string): RemoteToolProtocolError {
  try {
    const body = JSON.parse(text) as { error?: { code?: string; message?: string } };
    if (body.error?.code !== undefined) return new RemoteToolProtocolError(status, body.error.code, body.error.message ?? "请求失败");
  } catch {
    // 非 JSON 的代理错误也按 HTTP 错误表达；它不是一次可安全重试的未知结果。
  }
  return new RemoteToolProtocolError(status, "http_error", text || `HTTP ${status}`);
}

/** 只有明确的语义性 4xx 才停止；断网、429、5xx 都要带原 id 重试。 */
async function retryUnknown<T>(request: () => Promise<T>): Promise<T> {
  let lastError: unknown;
  for (let attempt = 0; attempt < RETRY_COUNT; attempt += 1) {
    try {
      return await request();
    } catch (error) {
      if (error instanceof RemoteToolProtocolError && error.status !== 429 && error.status < 500) throw error;
      lastError = error;
      if (attempt + 1 < RETRY_COUNT) await new Promise((resolve) => setTimeout(resolve, 25 * (attempt + 1)));
    }
  }
  throw lastError;
}
