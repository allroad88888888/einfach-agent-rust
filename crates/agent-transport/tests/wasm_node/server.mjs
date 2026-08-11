// 供 tests/wasm_smoke.rs 的 `wasm-pack test --node` 用例连的假 SSE 服务器。
// 跟 tests/it/fake_sse.rs 是同一件事的 Node 版本：那边手写 TcpListener 给
// native 的 ureq 客户端用，这边手写 http.Server 给 wasm 的 fetch 客户端用——
// 两边分别验证各自平台真实生产路径的分帧结果，互不复用代码是刻意的（各自
// 平台的真实网络栈本来就不同）。
//
// 用法：`node server.mjs <port>`，监听 127.0.0.1:<port>，几条路由对应
// fake_sse.rs 的场景：
//   /clean-close     一次性写完两行 SSE 数据后正常关闭连接
//   /slow-first-byte 先等 300ms 才开始写响应头（对应 023 的慢首字节场景）
//   /stall-forever   写一行后不再发数据、不关闭连接（配 abort 测试用）
//   /payment-required 402 + JSON 错误体，不该被重试
process.on("SIGTERM", () => process.exit(0));

import http from "node:http";

const port = Number(process.argv[2] || 0);

const server = http.createServer((req, res) => {
  const url = new URL(req.url, "http://127.0.0.1");
  switch (url.pathname) {
    case "/clean-close": {
      res.writeHead(200, { "Content-Type": "text/event-stream" });
      res.write('data: {"choices":[]}\n\n');
      setTimeout(() => {
        res.write("data: [DONE]\n\n");
        res.end();
      }, 50);
      break;
    }
    case "/slow-first-byte": {
      setTimeout(() => {
        res.writeHead(200, { "Content-Type": "text/event-stream" });
        res.write("data: [DONE]\n\n");
        res.end();
      }, 300);
      break;
    }
    case "/stall-forever": {
      res.writeHead(200, { "Content-Type": "text/event-stream" });
      res.write("data: first\n\n");
      // 故意不再写、不 end()——由客户端 abort 主动断开。
      req.on("close", () => {});
      break;
    }
    case "/payment-required": {
      const body = JSON.stringify({ error: { message: "Insufficient Balance" } });
      res.writeHead(402, {
        "Content-Type": "application/json",
        "Content-Length": Buffer.byteLength(body),
      });
      res.end(body);
      break;
    }
    default: {
      res.writeHead(404);
      res.end();
    }
  }
});

server.listen(port, "127.0.0.1", () => {
  // 唯一一行输出，供 run.sh 解析出实际监听端口。
  console.log(`LISTENING ${server.address().port}`);
});
