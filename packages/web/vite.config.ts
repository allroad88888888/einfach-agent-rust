// vite dev/build 配置。唯一非默认的东西是下面的 dev proxy——issue 033「注意」
// 原文：不给 agent-server 加 CORS，企业部署本来就走网关同源，dev 用代理模拟
// 同一种形状。`build` 用默认设置就够（静态产物，`pnpm --filter web build`）。
import { defineConfig } from "vite";

/** 代理目标：`AGENT_SERVER` 环境变量指定 agent-server 实际监听的地址（起
 * server 用的是 `crates/agent-server/examples/serve.rs`，默认随机端口，起来
 * 之后它自己会把真实地址打到 stderr——把那个地址喂给这个变量）。不设时退回
 * `http://127.0.0.1:4000`，跟 README「三步启动」里 `AGENT_SERVER_PORT=4000`
 * 的例子对应，图省事可以两边都不设环境变量、都用这个默认端口。*/
const target = process.env.AGENT_SERVER ?? "http://127.0.0.1:4000";

export default defineConfig({
  server: {
    proxy: {
      // 六个端点 + 会话创建/查询全部挂在 `/sessions` 前缀下（031 的路由表），
      // 一条代理规则够用，不用逐个端点列。
      "/sessions": {
        target,
        changeOrigin: true,
        // `GET /sessions/:id/events` 是长连接 SSE，不是 WebSocket——`ws` 保持
        // 默认 false；http-proxy 对 chunked 响应本来就是逐块透传，不需要额外
        // 关缓冲开关。
      },
    },
  },
  build: {
    outDir: "dist",
  },
});
