// 唯一职责：验收 web 执行器对远端工具 v2 认领与回执协议的行为。
import type { Frame, ToolCallRequest, ToolCallState } from "@agent/protocol";

import { registerWebTool } from "../src/capabilities/index";
import { createToolExecutor, fitToLimit } from "../src/tool-exec";
import { MAX_RESULT_BYTES, startMockServer, type MockServer, type Received } from "./tool-exec-mock-server";

let passed = 0;
const failures: string[] = [];
let sideEffects = 0;
const HUGE = "汉".repeat(600_000);

function check(label: string, condition: boolean, detail = ""): void {
  if (condition) { passed += 1; console.log(`  ✓ ${label}`); return; }
  failures.push(label);
  console.log(`  ✗ ${label}${detail ? ` —— ${detail}` : ""}`);
}

function eq(label: string, actual: unknown, expected: unknown): void {
  check(label, JSON.stringify(actual) === JSON.stringify(expected), `实际 ${JSON.stringify(actual)}，期望 ${JSON.stringify(expected)}`);
}

function request(tool: string): ToolCallRequest {
  return { tool, input: {}, location: "Web", reversibility: "Pure" };
}

function frame(callId: string, tool: string, agent = "root"): Frame {
  return { agent, event: { type: "tool_executing", data: { call_id: callId, request: request(tool) } } };
}

function dispatch(mock: MockServer, executor: (value: Frame) => void, callId: string, tool: string): void {
  mock.pending.set(callId, { agent: "root", request: request(tool), state: "pending_unclaimed" });
  executor(frame(callId, tool));
}

async function waitFor(received: Received[], count: number): Promise<void> {
  const until = Date.now() + 3000;
  while (received.length < count && Date.now() < until) await new Promise((resolve) => setTimeout(resolve, 10));
}

async function waitForCall(received: Received[], callId: string, path: string, count: number): Promise<void> {
  const until = Date.now() + 3000;
  while (received.filter((entry) => entry.path.endsWith(path) && entry.body.tool_call_id === callId).length < count && Date.now() < until) {
    await new Promise((resolve) => setTimeout(resolve, 10));
  }
}

function terminal(mock: MockServer, callId: string, state: Extract<ToolCallState, "failed" | "cancelled" | "outcome_unknown">): void {
  mock.pending.set(callId, { agent: "root", request: request("web:verify/counted"), state });
}

function registerTools(): void {
  registerWebTool({ name: "web:verify/counted", description: "有副作用", schema: {}, reversibility: "irreversible" }, () => {
    sideEffects += 1;
    return `第 ${sideEffects} 次`;
  });
  registerWebTool({ name: "web:verify/huge", description: "大文本", schema: {}, reversibility: "pure" }, () => HUGE);
  registerWebTool({ name: "web:verify/boom", description: "抛异常", schema: {}, reversibility: "pure" }, () => { throw new Error("故意炸的"); });
}

async function main(): Promise<void> {
  registerTools();
  const mock = await startMockServer();
  const realFetch = globalThis.fetch;
  globalThis.fetch = ((input: string | URL | Request, init?: RequestInit) => realFetch(typeof input === "string" ? new URL(input, mock.base) : input, init)) as typeof fetch;
  try {
    console.log("\n[1] UTF-8 1 MiB 上限");
    const cut = fitToLimit(HUGE);
    check("截断不超过 1 MiB", Buffer.byteLength(cut, "utf8") <= MAX_RESULT_BYTES);
    check("不切坏多字节字符", !cut.includes("�"));

    console.log("\n[2] 认领成功后才执行并收到同步 committed 确认");
    const one = createToolExecutor("s-1");
    dispatch(mock, one, "call-1", "web:verify/counted");
    await waitFor(mock.received, 2);
    eq("先 claim 再 result", mock.received.slice(0, 2).map((entry) => entry.path), ["/sessions/s-1/tool_claim", "/sessions/s-1/tool_result"]);
    eq("v2 result body", Object.keys(mock.received[1]?.body ?? {}).sort(), ["agent", "claim_id", "outcome", "submission_id", "tool_call_id"]);
    eq("副作用只做一次", sideEffects, 1);
    eq("终态是 succeeded", mock.pending.get("call-1")?.state, "succeeded");

    console.log("\n[3] 两执行器同时收到同一帧，只有原子认领赢家执行");
    const first = createToolExecutor("s-1");
    const second = createToolExecutor("s-1");
    dispatch(mock, first, "call-race", "web:verify/counted");
    second(frame("call-race", "web:verify/counted"));
    await waitFor(mock.received, 5);
    await new Promise((resolve) => setTimeout(resolve, 50));
    eq("竞态调用只做一次副作用", sideEffects, 2);
    eq("竞态终态正确", mock.pending.get("call-race")?.state, "succeeded");

    console.log("\n[4] HTTP 响应丢失时复用相同 submission_id，得到 duplicate 而不重做副作用");
    mock.dropNextResultResponse = true;
    dispatch(mock, one, "call-retry", "web:verify/counted");
    await waitFor(mock.received, 8);
    const retryResults = mock.received.filter((entry) => entry.path.endsWith("/tool_result") && entry.body.tool_call_id === "call-retry");
    eq("调用仍只执行一次", sideEffects, 3);
    eq("重试次数是两次", retryResults.length, 2);
    eq("两次使用同一 submission_id", retryResults[0]?.body.submission_id, retryResults[1]?.body.submission_id);
    eq("两次使用同一 claim_id", retryResults[0]?.body.claim_id, retryResults[1]?.body.claim_id);

    console.log("\n[5] claim 已生效但响应丢失时，以同一 claim_id 续领且只执行一次");
    mock.dropNextClaimResponse = true;
    dispatch(mock, one, "call-claim-drop", "web:verify/counted");
    await waitForCall(mock.received, "call-claim-drop", "/tool_result", 1);
    const droppedClaims = mock.received.filter((entry) => entry.path.endsWith("/tool_claim") && entry.body.tool_call_id === "call-claim-drop");
    eq("丢失响应后 claim 请求两次", droppedClaims.length, 2);
    eq("续领复用同一 claim_id", droppedClaims[0]?.body.claim_id, droppedClaims[1]?.body.claim_id);
    eq("already_claimed_by_you 后只执行一次", sideEffects, 4);

    console.log("\n[6] 找不到实现与实现异常都提交 failed，不沉默");
    dispatch(mock, one, "call-missing", "web:verify/missing");
    dispatch(mock, one, "call-boom", "web:verify/boom");
    await waitFor(mock.received, 15);
    eq("找不到工具是 failed", mock.pending.get("call-missing")?.state, "failed");
    eq("异常也是 failed", mock.pending.get("call-boom")?.state, "failed");
    const missing = mock.received.find((entry) => entry.path.endsWith("/tool_result") && entry.body.tool_call_id === "call-missing");
    eq("failed 结果不暴露旧 is_error", Object.keys((missing?.body.outcome ?? {}) as object).sort(), ["error", "status"]);

    console.log("\n[7] terminal / unknown 绝不再执行，并读状态以区分终态");
    const before = sideEffects;
    terminal(mock, "call-failed", "failed");
    terminal(mock, "call-cancelled", "cancelled");
    terminal(mock, "call-unknown", "outcome_unknown");
    one(frame("call-failed", "web:verify/counted"));
    one(frame("call-cancelled", "web:verify/counted"));
    one(frame("call-unknown", "web:verify/counted"));
    await new Promise((resolve) => setTimeout(resolve, 100));
    eq("三个终态都没有新增副作用", sideEffects, before);
    eq("三个 terminal 都查询状态", mock.statusQueries, 3);

    console.log("\n[8] sweep 用同一认领路径，不依赖 SSE 帧");
    mock.pending.set("call-sweep", { agent: "root", request: request("web:verify/counted"), state: "pending_unclaimed" });
    await one.sweep();
    await waitFor(mock.received, 20);
    eq("sweep 也执行了待办", sideEffects, before + 1);

    console.log("\n[9] 503 也复用原 id 重试；提交重试不会重新执行工具");
    mock.failNextClaimResponse = true;
    dispatch(mock, one, "call-claim-503", "web:verify/counted");
    await waitForCall(mock.received, "call-claim-503", "/tool_result", 1);
    const retriedClaims = mock.received.filter((entry) => entry.path.endsWith("/tool_claim") && entry.body.tool_call_id === "call-claim-503");
    eq("503 后 claim 请求两次", retriedClaims.length, 2);
    eq("503 后 claim_id 不变", retriedClaims[0]?.body.claim_id, retriedClaims[1]?.body.claim_id);
    eq("claim 重试后才执行一次", sideEffects, before + 2);

    mock.failNextResultResponse = true;
    dispatch(mock, one, "call-result-503", "web:verify/counted");
    await waitForCall(mock.received, "call-result-503", "/tool_result", 2);
    const retriedResults = mock.received.filter((entry) => entry.path.endsWith("/tool_result") && entry.body.tool_call_id === "call-result-503");
    eq("503 后 result 请求两次", retriedResults.length, 2);
    eq("503 后 submission_id 不变", retriedResults[0]?.body.submission_id, retriedResults[1]?.body.submission_id);
    eq("结果重试没有重做副作用", sideEffects, before + 3);
  } finally {
    globalThis.fetch = realFetch;
    await mock.close();
  }
  console.log(`\n=== ${passed} 条通过，${failures.length} 条失败 ===`);
  if (failures.length > 0) process.exitCode = 1;
}

void main();
