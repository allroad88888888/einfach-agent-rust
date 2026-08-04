// 唯一职责：一个**逐条复刻 agent-server 契约**的 mock 端点，给
// `verify-tool-exec.ts` 的断言当对手。断言本身不在这里（那是那个文件的职责）。
//
// 复刻的是两个端点：
//
// | 端点 | Rust 源 | 复刻了什么 |
// |---|---|---|
// | `POST /sessions/:id/tool_result` | `http/routes/tool_result.rs` | 同一个 1 MiB 上限（**UTF-8 字节**口径）、同一条「超了 400」、成功 202 Accepted |
// | `GET /sessions/:id/pending_tools` | `http/routes/pending_tools.rs` + `http/pending.rs` | 同一个 `{ pending: [{agent, call_id, request}] }` 形状（072） |
//
// 所以「截断之后还会不会被拒」「重放的那一帧还该不该执行」都是真的被判出来的，
// 不是读代码读出来的。
//
// **`pending` 这张表就是服务端的等待槽**（`RunnerCtx` 里那张）：测试自己往里
// 放（= `register_remote_tool`），收到回传就删掉（= `take_remote_tool` 取走）。
// 072 的整个判据建立在「投影跟槽同生同灭」上，mock 这一侧必须同样成立，否则
// 断言的是一个不存在的服务端。
import { createServer, type IncomingMessage } from "node:http";
import type { AddressInfo } from "node:net";

import type { ToolCallRequest } from "@agent/protocol";

/** 跟 `crates/agent-server/src/http/routes/tool_result.rs` 的 `MAX_RESULT_BYTES`
 * 同一个数。那边量的是 `String::len()`（UTF-8 字节），对应 Node 的
 * `Buffer.byteLength`——**不是** `.length`（UTF-16 码元）。 */
export const MAX_RESULT_BYTES = 1024 * 1024;

/** mock 收到的一次 `POST /tool_result`，连同它按服务端契约算出来的状态码。 */
export interface Received {
  path: string;
  status: number;
  body: { agent?: string; tool_call_id?: string; result?: { content?: string; is_error?: boolean } };
}

/** 一个还欠着的调用在 mock 侧的样子（`call_id` 是这张表的键，所以不在值里）。 */
export interface MockPending {
  agent: string;
  request: ToolCallRequest;
}

export interface MockServer {
  base: string;
  received: Received[];
  /** 服务端的等待槽。测试自己放/删，`GET /pending_tools` 读它。 */
  pending: Map<string, MockPending>;
  close: () => Promise<void>;
}

export async function startMockServer(): Promise<MockServer> {
  const received: Received[] = [];
  const pending = new Map<string, MockPending>();

  const server = createServer((req, res) => {
    void (async () => {
      if (req.method === "GET" && (req.url ?? "").endsWith("/pending_tools")) {
        const listing = [...pending.entries()].map(([call_id, owed]) => ({ agent: owed.agent, call_id, request: owed.request }));
        res.writeHead(200, { "Content-Type": "application/json" });
        res.end(JSON.stringify({ pending: listing }));
        return;
      }
      const raw = await readBody(req);
      const body = JSON.parse(raw) as Received["body"];
      const tooBig = Buffer.byteLength(body.result?.content ?? "", "utf8") > MAX_RESULT_BYTES;
      const status = tooBig ? 400 : 202;
      // 回传收下了 = 那个槽被 `take_remote_tool` 取走了，投影必须同步收缩。
      // 400 的那一支**不删**：server 那边根本没走到 `resolve_remote_tool`，
      // 槽还欠着（这正是 `tool-exec.ts` 要先截断再发的原因）。
      if (!tooBig && body.tool_call_id !== undefined) pending.delete(body.tool_call_id);
      received.push({ path: req.url ?? "", status, body });
      res.writeHead(status, { "Content-Type": "application/json" });
      res.end(tooBig ? '{"error":{"code":"bad_request","message":"tool result content 不能超过 1048576 bytes"}}' : "");
    })();
  });

  await new Promise<void>((resolve) => server.listen(0, "127.0.0.1", resolve));
  const { port } = server.address() as AddressInfo;
  return {
    base: `http://127.0.0.1:${port}`,
    received,
    pending,
    close: () =>
      new Promise<void>((resolve) => {
        server.closeAllConnections();
        server.close(() => resolve());
      }),
  };
}

function readBody(req: IncomingMessage): Promise<string> {
  return new Promise((resolve) => {
    let raw = "";
    req.setEncoding("utf8");
    req.on("data", (chunk: string) => (raw += chunk));
    req.on("end", () => resolve(raw));
  });
}
