# 058 Java 网关升级：拉取 Rust → 产生 SSE + 进程生命周期 ← M9 终点

**里程碑** M9 · **依赖** 055/056/057 · **模型** sonnet（+ 主会话真机 dogfood） · **独测** —（终点靠真实全链）

把三块拼成企业能直接抄的形态：网关**拉取** Rust、**产生** SSE 给浏览器、**持有** Rust 进程
的生命周期、按 **chatid** 路由。接缝见 [INTEGRATION.md](../INTEGRATION.md)。

## 范围（`examples/java-gateway/`）

1. **改成拉取式**：删掉「代理上游 SSE」那条路（`bodyToFlux(DataBuffer)` 透传那套），换成
   循环 `GET /sessions/{chatid}/events/poll`（带 `Last-Event-ID` + `X-Poll-Wait-Ms`），
   把拿到的 `frames` **产生**成给浏览器的 SSE。**四个坑随之消失**（不缓冲/不压缩/超时/取消
   传播全是「代理流」才有的问题），并在 README 里如实记「为什么现在不强制 WebFlux 了」。
   **推荐长轮询**（wait 20–25s）：guard 全程持有，既避开「轮询间隔必须 < 宽限（5s）」的
   约束，又把空转请求降到最低（057 §注意）。
2. **chatid 路由**：浏览器带 chatid → 网关 `POST /sessions {id: chatid}`（幂等，055 三态）
   → 拿到 200/201 → 开始 poll。**网关必须保证 chatid 的归属**（用户 A 不能拿到用户 B 的
   chatid）——这条是**部署契约、代码解决不了**，要在 README 显著位置写清楚，并给出
   「chatid 含 uuid」或「网关侧 `user → chatid` 授权校验」两条出路。
3. **进程生命周期**：`@PostConstruct` 用 `ProcessBuilder` 起 `agent-server`
   （`--port 0` + `--sessions-dir` + 每次启动独占的 `--ready-file`），读成功 bind 后
   原子发布的 `{"port":…,"pid":…,"version":…}` 拿真实端口，并核对 pid 后建 WebClient；
   不解析人类启动日志。`@PreDestroy` `process.destroy()`（Unix SIGTERM → Rust 侧**所有会话
   落盘快照之后才退**，超时才强杀）。Rust bin 提供这份最小 ready-file/SIGTERM 协议。
   README 写清代价：二进制分发（按平台打进 resources + 解压 chmod）、进程僵尸兜底、
   子进程 stdout/stderr 接进 Java 日志。
4. **正常关闭发 `POST /cancel`**（显式出路），宽限是兜底不是主路。

## 验收

**代码侧**（本 issue）：`mvn -q package` 已通过。若未来在没有 JDK 的环境中维护这份
参考实现，仍按 [037](037-java-gateway.md) 的既定处置：如实标注构建验证缺席，不能伪造
验收结果。

**真机 dogfood**（主会话跑，M9 终点）：

- 起 Java 网关（它自己拉起 Rust 子进程）→ 浏览器带 chatid 打开 → 真实对话可用、
  SSE 正常流。
- **同一 chatid 重开页面** → 历史还在（055 幂等 + 恢复）。
- **关掉浏览器** → 宽限后在飞轮次被取消（057，不白烧 token）。
- **停掉 Java 进程** → Rust 子进程**一起干净退出**（`@PreDestroy` → SIGTERM → 落盘），
  `ps` 无残留；重启 Java → 同 chatid 历史仍在。

## 注意

- **没有 JDK 的环境不要伪造构建结果**。这是 037 立下的规矩，必须如实记录验证边界。
- **`examples/java-gateway` 不是依赖**（决策 13）：不发 Maven、不跟版、README 第一句仍是
  「拷走改，别当依赖」。本 issue 只是让这份参考实现跟上新形态。
- **providers.toml 只读不印不提交**：真机 dogfood 要读它拿 key，任何输出只出长度/状态。
- **红线 8**：Rust 子进程绑 loopback（默认），网关不把它暴露出去。
- 真机若捞到新问题 → 单列新 issue，不塞进本 issue 硬修（049/050 的先例）。
- 收工验证前台跑完（WORKFLOW §四 -1）。
