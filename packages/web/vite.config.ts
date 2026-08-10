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

/** 私有 session API 的进程级 capability（`private_capability::matches` 在未配置
 * 时 fail-closed，全部 401）。真实部署由企业网关在转发时注入
 * `x-agent-server-capability` 头（issue 033「网关同源」）；本地 dev 用 vite 代理
 * 模拟同一种形状——`AGENT_CAPABILITY` 环境变量喂给代理，代理在转发时把该头拼
 * 到每个 `/sessions`、`/uploads` 请求上（浏览器侧不持有这个 secret，跟网关模型
 * 一致）。不设这个变量时行为一字不变（不注入任何头）。*/
const capability = process.env.AGENT_CAPABILITY;

function withCapabilityInjection() {
  if (!capability) {
    return {};
  }
  return {
    configure(proxy: { on: (event: string, handler: (req: any) => void) => void }) {
      proxy.on("proxyReq", (proxyReq: { setHeader: (name: string, value: string) => void }) => {
        proxyReq.setHeader("x-agent-server-capability", capability);
      });
    },
  };
}

export default defineConfig({
  server: {
    proxy: {
      // 六个端点 + 会话创建/查询全部挂在 `/sessions` 前缀下（031 的路由表），
      // 一条代理规则够用，不用逐个端点列。`GET /sessions/:id/events` 是长连接
      // SSE，不是 WebSocket——`ws` 保持默认 false；http-proxy 对 chunked 响应
      // 本来就是逐块透传，不需要额外关缓冲开关。
      "/sessions": {
        target,
        changeOrigin: true,
        ...withCapabilityInjection(),
      },
      // s5：传图链路的前一半 `POST /uploads` 也在 agent-server 上（multipart
      // 图片 → 临时目录 → `{"url":"/uploads/<id>"}`）。不代理的话 dev 模式下
      // 前端 `fetch("/uploads")` 会打在 vite 自己身上 404，传图直接断——生产
      // 同源托管（`AGENT_STATIC_DIR`）不受影响，这里是 dev 缺口的补齐。
      "/uploads": {
        target,
        changeOrigin: true,
        ...withCapabilityInjection(),
      },
    },
  },
  build: {
    outDir: "dist",
  },
});
