// 唯一职责：066 的**可判定验收**——把真的 `src/tool-exec.ts` 喂上真的
// `tool_executing` 帧，对着一个复刻了服务端契约的 mock 端点，逐条断言 issue
// 列的三件事（实现了的 → 202 且 `is_error:false` / 没实现的 → 回传 `is_error`
// 不沉默 / 超大结果截断后仍 202）。
//
//   pnpm --filter web verify:tool-exec
//
// 断言全绿退 0，任何一条红退 1。形状照抄 `verify-mcp.ts`（067）：本仓没有 TS
// 测试框架，Node 自带类型擦除 + `./ts-resolve.mjs` 就够，不为几十行断言往仓里
// 装 vitest。
//
// 072 起多一组 [8]：**重放不重复执行**。断言落在前端行为上，因为 bug 就在前端
// 行为上——server 侧那次迟到的回传本来就会被安全拒绝，坏的是副作用已经在宿主这边
// 真的发生了第二次。
//
// 复刻服务端契约的那个 mock 在 `./tool-exec-mock-server.ts`（`POST /tool_result`
// + `GET /pending_tools` + 那张等待槽表），本文件只管断言。
//
// 两处只有测试进程才需要的桥（浏览器里天然成立，Node 里没有）：
// 1. `api.ts` 发的是**相对路径**（`/sessions/...`，浏览器按页面同源解析）——
//    Node 的 `fetch` 解析不了相对 URL，这里把全局 `fetch` 包一层补上 base。
//    **不为此改产品代码**：真正跑在浏览器里的路径必须保持相对。
// 2. `capabilities/demo-tools.ts` 的两个示例工具读 `document`/`window`，Node
//    里没有——所以本文件另外 `registerWebTool` 几个只用得着纯 JS 的验收工具，
//    不碰示例工具（它们的真机验证是 065 记录里那一段）。
import type { Frame, ToolCallRequest } from "@agent/protocol";

import { registerWebTool } from "../src/capabilities/index";
import { createToolExecutor, fitToLimit } from "../src/tool-exec";
import { MAX_RESULT_BYTES, startMockServer, type MockServer, type Received } from "./tool-exec-mock-server";

let passed = 0;
const failures: string[] = [];

function check(label: string, condition: boolean, detail = ""): void {
  if (condition) {
    passed += 1;
    console.log(`  ✓ ${label}`);
  } else {
    failures.push(label);
    console.log(`  ✗ ${label}${detail ? ` —— ${detail}` : ""}`);
  }
}

function eq(label: string, actual: unknown, expected: unknown): void {
  const a = JSON.stringify(actual);
  const e = JSON.stringify(expected);
  check(label, a === e, `实际 ${a}，期望 ${e}`);
}

// ------------------------------------------------------------------ 验收用的工具

const HUGE = "汉".repeat(600_000); // 1.8 MB UTF-8，远超上限

/** 072：副作用计数器。`web:verify/counted` 的实现体**第一行**就加它——也就是
 * 真实世界里「下单」那一行。断言这个数，而不是断言下游可观测的结果：server 拒不
 * 拒这次迟到的回传跟副作用已经发生了几次完全无关（issue §突变论证）。 */
let sideEffects = 0;

function registerVerificationTools(): void {
  registerWebTool({ name: "web:verify/counted", description: "记一次副作用", schema: {}, reversibility: "irreversible" }, () => {
    sideEffects += 1;
    return `第 ${sideEffects} 次`;
  });
  registerWebTool({ name: "web:verify/echo", description: "回声", schema: {}, reversibility: "pure" }, (input) => `echo ${JSON.stringify(input)}`);
  registerWebTool({ name: "web:verify/boom", description: "总是抛", schema: {}, reversibility: "pure" }, () => {
    throw new Error("故意炸的");
  });
  registerWebTool({ name: "web:verify/huge", description: "吐一大坨", schema: {}, reversibility: "pure" }, () => HUGE);
  registerWebTool({ name: "web:verify/slow", description: "异步", schema: {}, reversibility: "pure" }, async () => {
    await new Promise((r) => setTimeout(r, 10));
    return "慢慢来";
  });
}

function webRequest(tool: string, location: ToolCallRequest["location"] = "Web"): ToolCallRequest {
  return { tool, input: {}, location, reversibility: "Pure" };
}

function frame(callId: string, tool: string, location: ToolCallRequest["location"] = "Web", agent = "root"): Frame {
  return { agent, event: { type: "tool_executing", data: { call_id: callId, request: webRequest(tool, location) } } };
}

/** 派一次活：**先在 mock 的等待槽里登记**，再把帧喂给执行器——服务端的顺序就是
 * 这个（`dispatch.rs` 的远端第五路：`register_remote_tool` 在 `emit` 的**上一行**，
 * 所以客户端拿着帧立刻来求证，问到的必然已经是含这条调用的新投影）。072 之后
 * 前端会拿帧去投影求证，槽里没登记就不执行。 */
function dispatch(mock: MockServer, executor: (f: Frame) => void, callId: string, tool: string, agent = "root"): void {
  mock.pending.set(callId, { agent, request: webRequest(tool) });
  executor(frame(callId, tool, "Web", agent));
}

/** 等到 mock 收够 `n` 条（或超时）——执行器是 fire-and-forget 的，没有可 await
 * 的句柄，这跟浏览器里的真实形状一致。 */
async function waitFor(received: Received[], n: number, ms = 3000): Promise<void> {
  const deadline = Date.now() + ms;
  while (received.length < n && Date.now() < deadline) await new Promise((r) => setTimeout(r, 10));
}

// ---------------------------------------------------------------------- 断言本体

async function main(): Promise<void> {
  registerVerificationTools();

  const mock = await startMockServer();
  const realFetch = globalThis.fetch;
  globalThis.fetch = ((input: string | URL | Request, init?: RequestInit) =>
    realFetch(typeof input === "string" ? new URL(input, mock.base) : input, init)) as typeof fetch;

  try {
    console.log("\n[1] 截断（纯函数，UTF-8 字节口径）");
    eq("没超限的原样返回", fitToLimit("短的"), "短的");
    const cut = fitToLimit(HUGE);
    check("截断后不超过 1 MiB", Buffer.byteLength(cut, "utf8") <= MAX_RESULT_BYTES, `${Buffer.byteLength(cut, "utf8")} 字节`);
    check("内容里说明了被截断", cut.includes("前端截断"), cut.slice(-80));
    check("刀口落在字符边界（没有 U+FFFD 残字）", !cut.includes("�"));
    check("保留的是开头那一段", cut.startsWith("汉汉汉"));

    console.log("\n[2] 实现了的工具 → 执行 → 202，is_error:false");
    const executor = createToolExecutor("s-1");
    dispatch(mock, executor, "call-1", "web:verify/echo");
    await waitFor(mock.received, 1);
    eq("打的是 tool_result 端点", mock.received[0]?.path, "/sessions/s-1/tool_result");
    eq("服务端契约：202 Accepted", mock.received[0]?.status, 202);
    eq("body 三个字段齐（没有 epoch）", Object.keys(mock.received[0]?.body ?? {}).sort(), ["agent", "result", "tool_call_id"]);
    eq("agent 原样带回", mock.received[0]?.body.agent, "root");
    eq("tool_call_id 原样带回", mock.received[0]?.body.tool_call_id, "call-1");
    eq("content 是实现的返回值（input 原样喂进去了）", mock.received[0]?.body.result?.content, "echo {}");
    eq("is_error:false", mock.received[0]?.body.result?.is_error, false);

    console.log("\n[3] 没实现的工具 → 回传 is_error，不沉默");
    dispatch(mock, executor, "call-2", "web:verify/不存在");
    await waitFor(mock.received, 2);
    eq("照样 202（server 收下了，loop 继续）", mock.received[1]?.status, 202);
    eq("is_error:true", mock.received[1]?.body.result?.is_error, true);
    check("content 说得出是哪个工具", (mock.received[1]?.body.result?.content ?? "").includes("web:verify/不存在"), mock.received[1]?.body.result?.content);

    console.log("\n[4] 实现抛异常 → is_error + 异常信息");
    dispatch(mock, executor, "call-3", "web:verify/boom");
    await waitFor(mock.received, 3);
    eq("is_error:true", mock.received[2]?.body.result?.is_error, true);
    check("带上了异常信息", (mock.received[2]?.body.result?.content ?? "").includes("故意炸的"), mock.received[2]?.body.result?.content);

    console.log("\n[5] 超大结果：前端先截断 → 仍然 202（不撞 400）");
    dispatch(mock, executor, "call-4", "web:verify/huge");
    await waitFor(mock.received, 4);
    eq("202，不是 400", mock.received[3]?.status, 202);
    check("mock 量到的字节数在上限内", Buffer.byteLength(mock.received[3]?.body.result?.content ?? "", "utf8") <= MAX_RESULT_BYTES);
    eq("截断不算失败", mock.received[3]?.body.result?.is_error, false);

    console.log("\n[6] 异步实现 + 不该执行的帧");
    dispatch(mock, executor, "call-5", "web:verify/slow");
    await waitFor(mock.received, 5);
    eq("await 到了返回值", mock.received[4]?.body.result?.content, "慢慢来");

    const before = mock.received.length;
    executor(frame("call-6", "srv:fs/read", "Server"));
    executor(frame("call-7", "desk:x/y", "Desktop"));
    executor({ agent: "root", event: { type: "text_delta", data: "不是工具帧" } });
    executor(frame("call-1", "web:verify/echo")); // 重放同一个 call_id
    await new Promise((r) => setTimeout(r, 200));
    eq("Server/Desktop/非工具帧/重放 一条都不发", mock.received.length, before);

    console.log("\n[7] 子 agent 的调用带回它自己的 agent");
    dispatch(mock, executor, "call-8", "web:verify/echo", "root/a1");
    await waitFor(mock.received, before + 1);
    eq("agent 是 root/a1，不是 root", mock.received[before]?.body.agent, "root/a1");

    // ---------------------------------------------------------------------
    console.log("\n[8] 072：重放不重复执行——判据是服务端的待办投影，不是「这帧是第几次见到」");
    // 爆炸半径是「同一个 chatid 上换一个**没有游标**的新客户端」（刷新 / 新 tab /
    // 网关重启在前端这一层长得一模一样：一份没有任何记忆的新实例）。它拿到的是
    // `replay(None)` 给的整个 ring，其中就有早已收场的那条 `tool_executing`。

    console.log("  —— [8.1] 派了活 → 执行 → 回传 → server 侧收场");
    const page1 = createToolExecutor("s-1");
    dispatch(mock, page1, "call-refresh", "web:verify/counted");
    await waitFor(mock.received, before + 2);
    eq("第一次派发真的执行了（副作用计数 1）", sideEffects, 1);
    eq("回传的是 call-refresh", mock.received.at(-1)?.body.tool_call_id, "call-refresh");
    check("mock 侧那条待办已经收场（投影里没了）", !mock.pending.has("call-refresh"));
    const settled = mock.received.length;

    console.log("  —— [8.2] 无游标的新客户端接上同一个会话，收到同一帧");
    // **必须重新 import**，不能只 new 一个 executor：只 new 的话「把 `handled`
    // 提成模块级单例」这种错的修法能骗过测试（Node 进程内模块状态不会因为 new
    // 一次就没）；真刷新连模块状态一起没。带 query 让 Node 重新求值这个模块。
    const { createToolExecutor: fresh } = await import("../src/tool-exec.ts?reload=2");
    const page2 = fresh("s-1");
    page2(frame("call-refresh", "web:verify/counted"));
    await new Promise((r) => setTimeout(r, 300)); // 给它足够时间去犯错
    eq("重放没有把副作用再做一次（仍是 1）", sideEffects, 1);
    eq("也没有第二条回传", mock.received.length, settled);

    console.log("  —— [8.3] 对照组：仍在等待中的调用必须被执行（漏活比重复执行更隐蔽）");
    dispatch(mock, page2, "call-owed", "web:verify/counted");
    await waitFor(mock.received, settled + 1);
    eq("还欠着的活照样执行（副作用计数 2）", sideEffects, 2);
    eq("回传的是 call-owed", mock.received.at(-1)?.body.tool_call_id, "call-owed");

    console.log("  —— [8.4] 帧根本到不了（ring 被挤爆的 gap / 断线期间派的活）：连上就拉待办也得干");
    const owedDone = mock.received.length;
    mock.pending.set("call-gap", { agent: "root", request: webRequest("web:verify/counted") });
    await page2.sweep(); // `main.ts` 在每次 `state === "open"` 上挂的就是这一条
    await waitFor(mock.received, owedDone + 1);
    eq("一帧都没喂，光靠投影也执行了（副作用计数 3）", sideEffects, 3);
    eq("回传的是 call-gap", mock.received.at(-1)?.body.tool_call_id, "call-gap");

    console.log("  —— [8.5] 同一条调用不会被「帧」和「拉待办」各干一次");
    const gapDone = mock.received.length;
    dispatch(mock, page2, "call-both", "web:verify/counted");
    await page2.sweep();
    await waitFor(mock.received, gapDone + 1);
    await new Promise((r) => setTimeout(r, 200));
    eq("只执行一次（副作用计数 4）", sideEffects, 4);
    eq("也只回传一次", mock.received.length, gapDone + 1);
  } finally {
    globalThis.fetch = realFetch;
    await mock.close();
  }

  console.log(`\n=== ${passed} 条通过，${failures.length} 条失败 ===`);
  if (failures.length > 0) {
    for (const f of failures) console.log(`  失败：${f}`);
    process.exitCode = 1;
  }
}

void main();
