// 唯一职责：为 tool-exec 验收复刻远端工具 v2 的最小 HTTP 状态机。
import { createServer, type IncomingMessage } from "node:http";
import type { AddressInfo } from "node:net";

import type { ToolCallRequest, ToolCallState, ToolOutcome } from "@agent/protocol";

export const MAX_RESULT_BYTES = 1024 * 1024;

export interface Received {
  path: string;
  status: number;
  body: Record<string, unknown>;
}

interface PendingCall {
  agent: string;
  request: ToolCallRequest;
  claimId?: string;
  state: ToolCallState;
  submission?: { id: string; outcome: ToolOutcome };
}

export interface MockServer {
  base: string;
  received: Received[];
  pending: Map<string, PendingCall>;
  statusQueries: number;
  failNextClaimResponse: boolean;
  failNextResultResponse: boolean;
  dropNextClaimResponse: boolean;
  dropNextResultResponse: boolean;
  close: () => Promise<void>;
}

export async function startMockServer(): Promise<MockServer> {
  const received: Received[] = [];
  const pending = new Map<string, PendingCall>();
  let statusQueries = 0;
  let failNextClaimResponse = false;
  let failNextResultResponse = false;
  let dropNextClaimResponse = false;
  let dropNextResultResponse = false;

  const server = createServer((req, res) => {
    void (async () => {
      const path = req.url ?? "";
      if (req.method === "GET" && path.includes("/pending_tools")) {
        return json(res, 200, { pending: [...pending.entries()]
          .filter(([, call]) => call.state === "pending_unclaimed" || call.state === "claimed")
          .map(([call_id, call]) => ({ agent: call.agent, call_id, request: call.request })) });
      }
      if (req.method === "GET" && path.includes("/tool_status")) {
        statusQueries += 1;
        const callId = new URL(path, "http://localhost").searchParams.get("tool_call_id") ?? "";
        const call = pending.get(callId);
        if (call === undefined) return error(res, 404, "tool_call_unknown");
        return json(res, 200, { state: call.state, revision: 1, retention_floor_revision: null, agent: call.agent, tool_call_id: callId, request: call.state === "pending_unclaimed" || call.state === "claimed" ? call.request : null, created_at_unix_ms: 1, updated_at_unix_ms: 1, deadline_at_unix_ms: null, claimed_by_me: false, submission_id: call.submission?.id ?? null, terminal_origin: call.state === "outcome_unknown" || call.state === "unclaimed_timeout" ? "deadline" : call.state === "cancelled" ? "session" : call.submission === undefined ? null : "host" });
      }
      const body = JSON.parse(await readBody(req)) as Record<string, unknown>;
      if (req.method === "POST" && path.endsWith("/tool_claim")) {
        const fail = failNextClaimResponse;
        failNextClaimResponse = false;
        const destroy = dropNextClaimResponse;
        dropNextClaimResponse = false;
        return claim(res, received, pending, path, body, fail, destroy);
      }
      if (req.method === "POST" && path.endsWith("/tool_result")) {
        const fail = failNextResultResponse;
        failNextResultResponse = false;
        const destroy = dropNextResultResponse;
        dropNextResultResponse = false;
        return result(res, received, pending, path, body, fail, destroy);
      }
      error(res, 404, "not_found");
    })();
  });

  await new Promise<void>((resolve) => server.listen(0, "127.0.0.1", resolve));
  const { port } = server.address() as AddressInfo;
  return {
    base: `http://127.0.0.1:${port}`,
    received,
    pending,
    get statusQueries() { return statusQueries; },
    get failNextClaimResponse() { return failNextClaimResponse; },
    set failNextClaimResponse(value: boolean) { failNextClaimResponse = value; },
    get failNextResultResponse() { return failNextResultResponse; },
    set failNextResultResponse(value: boolean) { failNextResultResponse = value; },
    get dropNextClaimResponse() { return dropNextClaimResponse; },
    set dropNextClaimResponse(value: boolean) { dropNextClaimResponse = value; },
    get dropNextResultResponse() { return dropNextResultResponse; },
    set dropNextResultResponse(value: boolean) { dropNextResultResponse = value; },
    close: () => new Promise<void>((resolve) => { server.closeAllConnections(); server.close(() => resolve()); }),
  };
}

function claim(res: import("node:http").ServerResponse, received: Received[], pending: Map<string, PendingCall>, path: string, body: Record<string, unknown>, fail: boolean, destroy: boolean): void {
  const callId = String(body.tool_call_id ?? "");
  const call = pending.get(callId);
  if (call === undefined) return error(res, 404, "tool_call_unknown");
  received.push({ path, status: 200, body });
  if (fail) return error(res, 503, "temporarily_unavailable");
  if (!isActive(call.state)) return error(res, 410, "tool_call_terminal");
  const claimId = String(body.claim_id ?? "");
  if (call.claimId !== undefined && call.claimId !== claimId) return error(res, 409, "tool_claimed_by_other");
  const disposition = call.claimId === undefined ? "claimed" : "already_claimed_by_you";
  call.claimId = claimId;
  call.state = "claimed";
  if (destroy) return res.destroy();
  json(res, 200, { disposition, agent: call.agent, tool_call_id: callId, request: call.request, revision: 1 });
}

function result(res: import("node:http").ServerResponse, received: Received[], pending: Map<string, PendingCall>, path: string, body: Record<string, unknown>, fail: boolean, destroy: boolean): void {
  const callId = String(body.tool_call_id ?? "");
  const call = pending.get(callId);
  const outcome = body.outcome as ToolOutcome | undefined;
  const content = outcome?.status === "succeeded" ? outcome.content : outcome?.status === "failed" ? outcome.error.message : outcome?.reason ?? "";
  if (Buffer.byteLength(content, "utf8") > MAX_RESULT_BYTES) return error(res, 400, "bad_request");
  if (call === undefined) return error(res, 404, "tool_call_unknown");
  received.push({ path, status: 200, body });
  if (fail) return error(res, 503, "temporarily_unavailable");
  if (!isActive(call.state)) {
    if (call.submission?.id === body.submission_id) return json(res, 200, receipt("duplicate", call, callId, String(body.submission_id)));
    return error(res, 410, "tool_call_terminal");
  }
  if (call.claimId !== body.claim_id) return error(res, 409, "tool_claim_required");
  call.submission = { id: String(body.submission_id), outcome: outcome! };
  call.state = outcome!.status;
  if (destroy) return res.destroy();
  json(res, 200, receipt("committed", call, callId, call.submission.id));
}

function receipt(disposition: string, call: PendingCall, callId: string, submissionId: string): object {
  return { disposition, terminal_status: call.state, agent: call.agent, tool_call_id: callId, submission_id: submissionId, revision: 2 };
}

function isActive(state: ToolCallState): boolean {
  return state === "pending_unclaimed" || state === "claimed";
}

function json(res: import("node:http").ServerResponse, status: number, body: object): void {
  res.writeHead(status, { "Content-Type": "application/json" });
  res.end(JSON.stringify(body));
}

function error(res: import("node:http").ServerResponse, status: number, code: string): void {
  json(res, status, { error: { code, message: code } });
}

function readBody(req: IncomingMessage): Promise<string> {
  return new Promise((resolve) => {
    let raw = "";
    req.setEncoding("utf8");
    req.on("data", (chunk: string) => { raw += chunk; });
    req.on("end", () => resolve(raw));
  });
}
