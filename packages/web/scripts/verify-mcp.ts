// 唯一职责：067 的**可判定验收**——对着 `./mock-mcp-server.ts` 跑一遍
// `src/mcp/`，逐条断言 issue 里列的四件事（翻译形状 / 版本协商不断言 /
// 通知被跳过 / 失败隔离）。
//
// 为什么是这个形状而不是 vitest：**本仓没有任何 TS 测试框架**
// （`packages/protocol/src/fixtures.test.ts` 那份「测试」的断言器就是 `tsc`
// 本身，头注释写得很清楚）。为跑几十行断言往仓里装 vitest + 一套配置，是给
// 别人加一个要维护的依赖；Node 24 自带类型擦除，`node` 能直接跑 `.ts`，只差
// 一个无扩展名 import 的解析钩子（`./ts-resolve.mjs`，12 行）。
//
//   pnpm --filter web verify:mcp
//
// 断言全绿退 0，任何一条红退 1。
import { startMockMcpServer, unreachableUrl } from "./mock-mcp-server";
import { connectMcpServers, McpClient, registerMcpTools } from "../src/mcp/index";
import { CLIENT_PROTOCOL_VERSION } from "../src/mcp/protocol";
import { flattenToolResult } from "../src/mcp/tool_result";
import { injectedToolName, parseInjectedToolName, translateTool } from "../src/mcp/translate";
import { webCapabilities } from "../src/capabilities/index";

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

// ---------------------------------------------------------------- 纯函数部分

function pureFunctionChecks(): void {
  console.log("\n[1] 翻译规则（纯函数，对应 crates/agent-mcp/src/translate.rs）");

  const base = { name: "t", description: "d", inputSchema: { type: "object" }, annotations: null };
  eq("readOnlyHint: true → pure", translateTool({ ...base, annotations: { readOnlyHint: true } }, "s").reversibility, "pure");
  eq("readOnlyHint: false → irreversible", translateTool({ ...base, annotations: { readOnlyHint: false } }, "s").reversibility, "irreversible");
  eq("没有 annotations → irreversible", translateTool(base, "s").reversibility, "irreversible");
  eq("annotations 里没有 readOnlyHint → irreversible", translateTool({ ...base, annotations: { readOnlyHint: null } }, "s").reversibility, "irreversible");

  eq("名字形如 web:mcp-<server>/<tool>", translateTool({ ...base, name: "echo" }, "figma").name, "web:mcp-figma/echo");
  eq("描述缺失落空串，不拿名字顶替", translateTool({ ...base, description: null }, "s").description, "");
  eq("schema 原样搬", translateTool({ ...base, inputSchema: { type: "object", properties: { a: { type: "string" } } } }, "s").schema, {
    type: "object",
    properties: { a: { type: "string" } },
  });

  console.log("\n[2] 名字拆解（066 路由用）");
  eq("拆回 (server, tool)", parseInjectedToolName(injectedToolName("figma", "get_file")), { serverId: "figma", toolName: "get_file" });
  eq("服务端 MCP 的名字不归本模块管", parseInjectedToolName("mcp:everything/echo"), null);
  eq("前端自有工具不归本模块管", parseInjectedToolName("web:demo/page-title"), null);
  eq("缺斜杠 → null", parseInjectedToolName("web:mcp-figma"), null);
  eq("非法字符 → null（不 sanitize）", parseInjectedToolName("web:mcp-fig ma/x"), null);

  console.log("\n[3] tools/call result 拍平（066 直接拿去组 tool_result）");
  eq("多个 text 块拼接", flattenToolResult({ content: [{ text: "a" }, { text: "b" }] }), { text: "a\nb", isError: false });
  eq("isError 读出来", flattenToolResult({ content: [{ text: "x" }], isError: true }), { text: "x", isError: true });
  check("没有 text 块时不喂空串", flattenToolResult({ content: [{ type: "image", data: "..." }] }).text.includes("image"));
}

// ------------------------------------------------------------------ 连真的 mock

async function liveChecks(): Promise<void> {
  const mock = await startMockMcpServer("2099-01-01");
  const dead = await unreachableUrl();

  try {
    console.log("\n[4] 版本协商：记录，不断言相等");
    const client = await McpClient.connect({ url: mock.url, clientName: "verify", clientVersion: "0" });
    eq("采用 server 回的版本", client.protocolVersion, "2099-01-01");
    check("跟客户端提议的版本确实不同（这一步没抛，就是不断言）", client.protocolVersion !== CLIENT_PROTOCOL_VERSION);
    eq("serverInfo.name 读出来", client.serverName, "mock");
    await client.close();

    console.log("\n[5] 失败隔离：一个连不上，另一个照常");
    const source = await connectMcpServers([
      { id: "demo", url: mock.url },
      { id: "broken", url: dead },
      { id: "demo", url: mock.url },
      { id: "bad id!", url: mock.url },
    ]);

    eq("四条配置四条状态", source.servers.length, 4);
    eq("好的那个连上了", source.servers[0]?.availability.kind, "connected");
    eq("坏的那个标 unavailable", source.servers[1]?.availability.kind, "unavailable");
    check(
      "unavailable 带得出原因（不静默）",
      (source.servers[1]?.availability as { reason?: string }).reason !== undefined &&
        (source.servers[1]?.availability as { reason: string }).reason.length > 0,
      JSON.stringify(source.servers[1]),
    );
    eq("重复 id 不装载", source.servers[2]?.availability.kind, "unavailable");
    eq("非法 id 不装载", source.servers[3]?.availability.kind, "unavailable");

    console.log("\n[6] tools/list：SSE 里的通知/错 id 被跳过，工具翻成注入声明");
    const names = source.tools.map((t) => t.name);
    eq("六个工具里跳掉名字非法的那个，剩五个", source.tools.length, 5);
    eq("名字全部形如 web:mcp-demo/<tool>", names, [
      "web:mcp-demo/echo",
      "web:mcp-demo/write_file",
      "web:mcp-demo/no_annotations",
      "web:mcp-demo/empty_annotations",
      "web:mcp-demo/undescribed",
    ]);
    eq("readOnlyHint: true → pure", source.tools[0]?.reversibility, "pure");
    eq("readOnlyHint: false → irreversible", source.tools[1]?.reversibility, "irreversible");
    eq("无 annotations → irreversible", source.tools[2]?.reversibility, "irreversible");
    eq("无 readOnlyHint → irreversible", source.tools[3]?.reversibility, "irreversible");
    eq("无描述 → 空串", source.tools[4]?.description, "");
    check("名字非法的工具没混进来", !names.some((n) => n.includes("bad")));

    const listRequest = mock.seen.find((r) => r.rpcMethod === "tools/list");
    eq("后续请求回带 Mcp-Session-Id", listRequest?.sessionId, "mock-session-1");
    eq("后续请求带协商后的 MCP-Protocol-Version", listRequest?.protocolVersion, "2099-01-01");
    check("Accept 两种都声明", (listRequest?.accept ?? "").includes("text/event-stream"));

    console.log("\n[7] 路由与执行");
    check("handles 认自己的", source.handles("web:mcp-demo/echo"));
    check("handles 不认服务端 MCP", !source.handles("mcp:everything/echo"));
    check("handles 不认没连上的源", !source.handles("web:mcp-broken/x"));

    const ok = await source.call("web:mcp-demo/echo", { msg: "hi" });
    eq("调用结果拍平成文本", ok, { text: 'echo: {"msg":"hi"}', isError: false });
    eq("MCP 报 isError → 原样带回", await source.call("web:mcp-demo/write_file", {}), { text: "磁盘满了", isError: true });

    const rpcErr = await source.call("web:mcp-demo/no_annotations", {});
    check("JSON-RPC error 落成 isError，不抛", rpcErr.isError && rpcErr.text.includes("未知工具"), JSON.stringify(rpcErr));

    let threw = false;
    try {
      await source.call("web:demo/page-title", {});
    } catch {
      threw = true;
    }
    check("不归自己管的名字要抛（那是调用方路由 bug，不该变成一次工具失败）", threw);

    console.log("\n[8] 与 065 的接线：registerWebTool");
    const registered = registerMcpTools(source);
    eq("五个都登记进去了", registered, 5);
    const declared = webCapabilities()?.tools?.map((t) => t.name) ?? [];
    check("MCP 工具进了 webCapabilities()", declared.includes("web:mcp-demo/echo"), JSON.stringify(declared));
    check("065 自己的示例工具还在", declared.includes("web:demo/page-title"), JSON.stringify(declared));

    await source.close();
    check("close() 不抛", true);
  } finally {
    await mock.close();
  }
}

async function main(): Promise<void> {
  pureFunctionChecks();
  await liveChecks();

  console.log(`\n=== ${passed} 条通过，${failures.length} 条失败 ===`);
  if (failures.length > 0) {
    for (const f of failures) console.log(`  失败：${f}`);
    process.exitCode = 1;
  }
}

void main();
