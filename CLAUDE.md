# CLAUDE.md

企业级 Agent 运行时。核心是一个**原子状态引擎**——agent 的全部状态活在一张依赖图里，
因此 undo / redo / 崩溃恢复 / 审计回放是同一套机制的四个投影，而不是四个功能。

**整棵 agent 树（root + 所有子 agent）共用一个 store**，靠 family 的 `AgentId` 区分实例。
于是子 agent 读父状态是一次 `get`，等待子 agent 完成是一个 derived atom，跨 agent 的 undo
天生一致。见 [docs/STATE-MODEL.md](docs/STATE-MODEL.md) §「子 agent」。

## 文档地图

| 文档 | 管什么 |
|---|---|
| [docs/ROADMAP.md](docs/ROADMAP.md) | **已拍板的决策、现状、阶段顺序、未决问题** |
| [docs/issues/](docs/issues/README.md) | **逐条任务**，一个文件一个，带验收标准 |
| [docs/WORKFLOW.md](docs/WORKFLOW.md) | 怎么做一个 issue：粒度、用什么模型、测试谁写 |
| [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) | 包结构、传输、部署形态、各包边界 |
| [docs/ADAPTER.md](docs/ADAPTER.md) | **模型适配层的接缝定义**：料单 / 能力位 / trait / 放错的症状 |
| [docs/STATE-MODEL.md](docs/STATE-MODEL.md) | 原子图、undo/redo、持久化与恢复 |
| [docs/TOOLS.md](docs/TOOLS.md) | 工具三分与位置透明路由、skills、MCP |
| [docs/INVARIANTS.md](docs/INVARIANTS.md) | **红线**——违反了整套机制就是漏的 |
| [probes/PROVIDERS.md](probes/PROVIDERS.md) | 三家模型的实测差异（adapter 内部消化，主线别引用细节） |

M5 之后每个里程碑各留一份**接缝文档**，按需读，别一次全读：

| 文档 | 接缝管什么 | 里程碑 |
|---|---|---|
| [docs/MCP.md](docs/MCP.md) | 外部工具来源的差异在哪消化 | M6 |
| [docs/OBSERVABILITY.md](docs/OBSERVABILITY.md) | 子 agent 状态**给人看** | M7 |
| [docs/ORCHESTRATION.md](docs/ORCHESTRATION.md) | 子 agent 状态**给模型看** | M8 |
| [docs/INTEGRATION.md](docs/INTEGRATION.md) | 给**企业宿主**看：chatid 身份 / 拉取式传输 / 进程生命周期 | M9 |
| [docs/HOST-CAPABILITIES.md](docs/HOST-CAPABILITIES.md) | 宿主声明自己的 tool / skill / MCP | M10 |

新会话先读 `ROADMAP.md`（知道在哪）、`docs/issues/`（知道下一步做什么）、
`INVARIANTS.md`（知道什么不能碰）。
其余按需。

## 红线摘要

完整版与理由见 [docs/INVARIANTS.md](docs/INVARIANTS.md)。这里只列条目，**不要凭摘要动手**：

1. derived 的 read fn 必须是纯函数
2. 业务代码禁止直接调 `store.set()`，一律走 command 层
3. primitive atom 的值必须全部可序列化
4. 快照与日志落盘用 `AtomKey`，不用 `AtomId`
5. 大值必须 `Arc` 包住，`PartialEq` 走 `ptr_eq` 快路
6. 在飞的 effect 必须带 epoch，回写前校验
7. `agent-core` / `agent-store` 不得做 IO
8. `bind` 地址默认 `127.0.0.1`
9. 文件行数：普通 ≤300，复杂 ≤500
10. agent 之间只允许上下读，禁止横读
11. 会进 prompt 的东西，序列化必须逐字节确定（禁 `HashMap`/`HashSet`）
12. **core 里不许有任何模型相关的判断**——没有 `match provider`，也没有 `if caps.xxx()`

1–6 条错了不会立刻报错，会在 undo 或崩溃恢复时以静默错值的形式浮出来。第 11 条同理 ——
功能完全正常，只是每一轮都全价（DeepSeek 上 120 倍）。第 12 条也一样静默 ——
一直正常到加第四家 provider 时发现要改 core。这是本仓最贵的几类 bug，
所以它们是红线不是建议。

## 当前状态

**M1–M9 全部完成**（2026-08-01 ~ 08-04），**M10 宿主能力注入进行中**。

同一个核心库的四种形态都真实验收过：CLI（undo/恢复/屏障）、浏览器（SSE/多 agent 并行/
断开取消）、独立 server bin、桌面 app（内嵌同库同前端）。M5 skills 装载、M6 MCP、
M7 子 agent 可观测、M8 模型侧异步编排、M9 企业集成**各有真机 dogfood 验收**（真 provider，
不是 mock），逐条兑现记录在 `docs/ROADMAP.md` §二和各 issue 的实做记录里。

两条最容易过期的事实：**Java 参考网关已构建验证**（OpenJDK 21 + Maven 3.9.15，037 那句
「本机无 JDK」已被 M9 推翻），M9 起它是**拉取式**——网关 poll Rust、自己产生 SSE 给浏览器，
并用 `ProcessBuilder` + `--ready-file` 拉起 Rust 子进程（[docs/INTEGRATION.md](docs/INTEGRATION.md)）。

M10 在做「前端/网关声明自己的 tool、skill、MCP」，接缝见 `docs/HOST-CAPABILITIES.md`。
核心判断：**不需要新机制，只缺一个声明入口**——注入的能力跟自有的走完全相同的路。

动手前仍然先 `ls crates/` + `cargo test` 确认现状，别信文档对「已完成」的描述——
[docs/WORKFLOW.md](docs/WORKFLOW.md) §四第 0 步（这也是这段不写测试数的原因：它必然过期）。
明确未排期的：多租户、多副本 `RedisRegistry`（ARCHITECTURE §多副本是**草案，未实现**）、
MCP 的 OAuth / resources / prompts——都等真实使用反馈再定，别提前猜。

## 上游血缘

`crates/agent-store`（M2 才建）fork 自 [einfach](https://github.com/allroad88888888/einfach) 的
Rust 原子引擎（`excel/rust/core`，crate 名 `einfach-core`）。fork 之后**独立演进**，
不回合上游，也不同步上游的 bug 修复——需要移植时手工挑。

fork 时移除的 Excel 血统：`ArrayData`（rows×cols 矩形块）、`LambdaValue`、Excel 错误码。
保留不动的：同步可重入语义、pending 队列的 glitch-free 传播、256 深度预算、`AtomFamily`。

## Workspace

主 Cargo workspace 在 `crates/`（**十个 crate**，`Cargo.toml` 的 members 是权威），
pnpm workspace 在 `packages/`（`protocol` + `web`）+ `apps/`（`desktop`，M3 建）。
另有两个**独立 workspace**，不进主依赖图：`probes/api` 与 `apps/desktop/src-tauri`。
TS 侧的协议类型由 Rust 用 **ts-rs** 生成，**不手写**——见 ARCHITECTURE.md §「协议类型」。

## 自动检查

`scripts/check-invariants.sh` 挂在 Edit/Write 的 PostToolUse hook 上，检查能被 grep
判定的红线（行数、禁用依赖、`store.set`、`AtomId` 序列化、derived 里的时钟/随机）。
需要判断的部分（这个 atom 该 primitive 还是 derived、这个 tool 的 reversibility 等级怎么定）
走 skill `agent-state-design`。

CI 上跑同一个脚本：`scripts/check-invariants.sh --all`。
