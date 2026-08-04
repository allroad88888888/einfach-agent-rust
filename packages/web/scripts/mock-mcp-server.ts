// 唯一职责：一个**最小 mock MCP server**（Streamable HTTP 传输），只服务
// `scripts/verify-mcp.ts` 的断言。不是产品代码，不在 `src/` 下、不进 tsconfig
// 的 `include`、不进 vite 的产物。
//
// 它刻意做了三件「真 server 会干、而客户端必须扛住」的事：
// 1. `initialize` 回一个**跟客户端提议的不一样**的 protocolVersion；
// 2. `tools/list` 走 `text/event-stream`，并在真响应之前**插播一条通知**、
//    再插一条 **id 不对号**的响应，真响应本身还拆成**两行 `data:`**；
// 3. 带 `Mcp-Session-Id`，后续请求必须回带。
import { createServer, type IncomingMessage, type ServerResponse } from "node:http";
import type { AddressInfo } from "node:net";

export interface SeenRequest {
  httpMethod: string;
  rpcMethod: string | null;
  sessionId: string | null;
  accept: string | null;
  protocolVersion: string | null;
}

export interface MockServer {
  url: string;
  seen: SeenRequest[];
  close: () => Promise<void>;
}

/** `tools/list` 的固定清单——覆盖可逆性翻译的全部四种取值，外加一个描述缺失
 * 的、一个名字含非法字符的。 */
const TOOLS = [
  { name: "echo", description: "回声", inputSchema: { type: "object" }, annotations: { readOnlyHint: true } },
  { name: "write_file", description: "写文件", inputSchema: { type: "object" }, annotations: { readOnlyHint: false } },
  { name: "no_annotations", description: "没有 annotations", inputSchema: { type: "object" } },
  { name: "empty_annotations", description: "annotations 里没有 readOnlyHint", inputSchema: { type: "object" }, annotations: {} },
  { name: "undescribed", inputSchema: { type: "object", properties: { a: { type: "string" } } } },
  { name: "bad name!", description: "名字含非法字符，应当被跳过", inputSchema: { type: "object" } },
];

const SESSION_ID = "mock-session-1";

export async function startMockMcpServer(protocolVersion = "2099-01-01"): Promise<MockServer> {
  const seen: SeenRequest[] = [];

  const server = createServer((req, res) => {
    void handle(req, res, seen, protocolVersion);
  });

  await new Promise<void>((resolve) => server.listen(0, "127.0.0.1", resolve));
  const { port } = server.address() as AddressInfo;

  return {
    url: `http://127.0.0.1:${port}/mcp`,
    seen,
    close: () =>
      new Promise<void>((resolve) => {
        server.closeAllConnections();
        server.close(() => resolve());
      }),
  };
}

/** 起一个 server 再立刻关掉，拿到一个**确定连不上**的地址（失败隔离用）。 */
export async function unreachableUrl(): Promise<string> {
  const server = await startMockMcpServer();
  const url = server.url;
  await server.close();
  return url;
}

async function handle(req: IncomingMessage, res: ServerResponse, seen: SeenRequest[], protocolVersion: string): Promise<void> {
  const body = await readBody(req);
  const message = body === "" ? null : (JSON.parse(body) as { id?: number; method?: string; params?: Record<string, unknown> });

  seen.push({
    httpMethod: req.method ?? "",
    rpcMethod: message?.method ?? null,
    sessionId: header(req, "mcp-session-id"),
    accept: header(req, "accept"),
    protocolVersion: header(req, "mcp-protocol-version"),
  });

  if (req.method === "DELETE") {
    res.writeHead(200).end();
    return;
  }
  if (message === null || message.method === undefined) {
    res.writeHead(400).end("不是 JSON-RPC");
    return;
  }

  switch (message.method) {
    case "initialize":
      json(res, {
        jsonrpc: "2.0",
        id: message.id,
        // 故意跟客户端提议的版本不一样——客户端必须记录而不是断言。
        result: { protocolVersion, capabilities: { tools: {} }, serverInfo: { name: "mock", version: "0.0.0" } },
      }, { "Mcp-Session-Id": SESSION_ID });
      return;

    case "notifications/initialized":
      res.writeHead(202).end();
      return;

    case "tools/list":
      toolsListOverSse(res, message.id ?? 0);
      return;

    case "tools/call":
      toolsCall(res, message.id ?? 0, message.params ?? {});
      return;

    default:
      json(res, { jsonrpc: "2.0", id: message.id, error: { code: -32601, message: `未知方法 ${message.method}` } });
  }
}

/** 真响应之前插播：一条注释、一条通知、一条 id 不对号的响应；真响应本身拆成
 * 两行 `data:`（规范说多行 `data:` 用 `\n` 拼接）。 */
function toolsListOverSse(res: ServerResponse, id: number): void {
  res.writeHead(200, { "Content-Type": "text/event-stream; charset=utf-8", "Cache-Control": "no-cache" });
  res.write(": keep-alive\n\n");
  res.write(`data: ${JSON.stringify({ jsonrpc: "2.0", method: "notifications/tools/list_changed" })}\n\n`);
  res.write(`data: ${JSON.stringify({ jsonrpc: "2.0", id: 9999, result: { tools: [] } })}\n\n`);

  const real = JSON.stringify({ jsonrpc: "2.0", id, result: { tools: TOOLS } });
  // 在第一个 `{` 之后断开：拼回来时中间那个换行是 JSON 的合法空白。
  res.write(`data: ${real.slice(0, 1)}\ndata: ${real.slice(1)}\n\n`);
  res.end();
}

function toolsCall(res: ServerResponse, id: number, params: Record<string, unknown>): void {
  const name = params.name;
  const args = params.arguments;

  if (name === "echo") {
    json(res, {
      jsonrpc: "2.0",
      id,
      result: { content: [{ type: "text", text: `echo: ${JSON.stringify(args)}` }] },
    });
    return;
  }
  if (name === "write_file") {
    json(res, { jsonrpc: "2.0", id, result: { content: [{ type: "text", text: "磁盘满了" }], isError: true } });
    return;
  }
  json(res, { jsonrpc: "2.0", id, error: { code: -32602, message: `未知工具 ${String(name)}` } });
}

function json(res: ServerResponse, payload: unknown, extraHeaders: Record<string, string> = {}): void {
  res.writeHead(200, { "Content-Type": "application/json", ...extraHeaders });
  res.end(JSON.stringify(payload));
}

function header(req: IncomingMessage, name: string): string | null {
  const value = req.headers[name];
  return typeof value === "string" ? value : null;
}

async function readBody(req: IncomingMessage): Promise<string> {
  const chunks: Buffer[] = [];
  for await (const chunk of req) chunks.push(chunk as Buffer);
  return Buffer.concat(chunks).toString("utf8");
}
