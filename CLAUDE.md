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
| [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) | 包结构、传输、部署形态、各包边界（英译 [ARCHITECTURE.en.md](docs/ARCHITECTURE.en.md) 并存，**中文是权威**） |
| [docs/ADAPTER.md](docs/ADAPTER.md) | **模型适配层的接缝定义**：料单 / 能力位 / trait / 放错的症状 |
| [docs/STATE-MODEL.md](docs/STATE-MODEL.md) | 原子图、undo/redo、持久化与恢复（英译 [STATE-MODEL.en.md](docs/STATE-MODEL.en.md) 并存，**中文是权威**） |
| [docs/TOOLS.md](docs/TOOLS.md) | 工具三分与位置透明路由、skills、MCP |
| [docs/INVARIANTS.md](docs/INVARIANTS.md) | **红线**——违反了整套机制就是漏的（英译 [INVARIANTS.en.md](docs/INVARIANTS.en.md) 并存，**中文是权威**） |
| [probes/PROVIDERS.md](probes/PROVIDERS.md) | 三家模型的实测差异（adapter 内部消化，主线别引用细节） |

M5 之后每个里程碑各留一份**接缝文档**，按需读，别一次全读：

| 文档 | 接缝管什么 | 里程碑 |
|---|---|---|
| [docs/MCP.md](docs/MCP.md) | 外部工具来源的差异在哪消化 | M6 |
| [docs/OBSERVABILITY.md](docs/OBSERVABILITY.md) | 子 agent 状态**给人看** | M7 |
| [docs/ORCHESTRATION.md](docs/ORCHESTRATION.md) | 子 agent 状态**给模型看** | M8 |
| [docs/INTEGRATION.md](docs/INTEGRATION.md) | 给**企业宿主**看：chatid 身份 / 拉取式传输 / 进程生命周期 | M9 |
| [docs/HOST-CAPABILITIES.md](docs/HOST-CAPABILITIES.md) | 宿主声明自己的 tool / skill / MCP | M10 |
| [docs/IMAGES.md](docs/IMAGES.md) | 图片附件：怎么进 prompt、吃不下的家怎么降级 | M11 |
| [docs/issues/119-browser-host-capability-decision.md](docs/issues/119-browser-host-capability-decision.md) | 浏览器宿主的通用工具回调与图片：JS/Rust 分工在哪切 | M14 |
| [docs/EXTENSIONS.md](docs/EXTENSIONS.md) | Rust 扩展包：交付物形状、两阶段装配、`Session` 手套的能与不能 | M16 |

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
10. 跨 agent 的边只许指向 primitive（读**不限方向**，兄弟互读是允许的）
11. 会进 prompt 的东西，序列化必须逐字节确定（禁 `HashMap`/`HashSet`）
12. **core 里不许有任何模型相关的判断**——没有 `match provider`，也没有 `if caps.xxx()`

1–6 条错了不会立刻报错，会在 undo 或崩溃恢复时以静默错值的形式浮出来。第 11 条同理 ——
功能完全正常，只是每一轮都全价（DeepSeek 上 120 倍）。第 12 条也一样静默 ——
一直正常到加第四家 provider 时发现要改 core。这是本仓最贵的几类 bug，
所以它们是红线不是建议。

## 当前状态

**M20（多 agent 网格，决策 35）2026-08-18 落地**：横读全开（红线 10 从「方向」改成
「边只许指向 primitive」）、`srv:agent/send` 两档送达时机、`srv:agent/self`、
`srv:agent/notes` 草稿纸、`srv:agent/await` + 等待图、**以及会话第一次能在没有用户
输入的情况下自己往下跑**（`--max-auto-turns`，只有真实用户输入能把预算加满）。
最后一样比新增几个工具重要得多：三道量不同东西的闸（树多大 / 一轮说几次话 /
没人看着时跑几轮）**相乘**才是「用户按一次回车之后最坏花多少钱」。

**M1–M19 全部完成**（2026-08-01 ~ 08-18，无尾巴）；**L 波（对外推广）2026-08-13
推进到「只剩用户动作」**——见 [docs/issues/README.md](docs/issues/README.md) §L。
L 波带出五条进主线的改动：`openai` 通用兼容 adapter（决策 33，ROADMAP §一/§二）、
CI 门禁复活（`.github/workflows/ci.yml`，三个 job）、Pages 部署
（`pages.yml` → https://allroad88888888.github.io/einfach-agent-rust/ ，
**浏览器 demo 现已公开可点**）、**`einfach-store 0.1.0` 已上 crates.io**（2026-08-13，issue 182；发布走
`release.yml`，打 `einfach-store-v*` tag 触发，token 只作为 GitHub secret 存在，
不落本机——升版本照走这条）、
以及 `scripts/clean-build-cache.sh` +
`scripts/check-build-cache.sh`（构建缓存 35G→9G，见 §Workspace）。

M19 可逆性从标签改成交付物（决策 34，199–203，2026-08-17~18）：**「这一步能不能撤销」
从一个声明的枚举变成一个交回来的函数**。工具执行完交回三态 `Aftermath`
（`Nothing`/`Undo(f)`/`Irreversible`），译成 core 记账的三态 `Undoability`
（`StateOnly`/`Hooked`/`Blocked`）；undo 路上还原钩子在回滚状态**之前**逐条逆序跑。
起点是一次清账：`Reversibility::Reversible` 当时唯一的差别是打印给人看的字符串，
宿主声明 `reversible` 的工具 `/undo` 会静默跳过、副作用原样留着。**执行体在别的进程里
的两类**（宿主 `web:`/`desk:`、MCP）交不回函数，判据是**声明的是事实还是承诺**：
`pure`/`readOnlyHint:true` 是事实，采信不挡；`reversible` 是交不出的承诺，挡。
`Reversibility` 枚举自此**只是显示标签**——看到任何拿它当 undo 行为依据的说法，那是过期的。

M18 子 agent 上限
的配置面（决策 32）：决策 20 的两道闸从「代码可配、运行时无入口」变成进程级启动参数
（`--max-agent-depth`/`--max-children`，env 兜底），协议面零改动；顺带堵掉恢复路径上
一处静默失配（`recover` 补 `limits` 入参，160）。真机七条全过，最硬的一条是 `kill -9`
恢复后闸仍是配的值而不是默认档。M17 宿主声明开局块
（决策 31）：`capabilities.prefix` 声明**内容**不声明执行体，装配期合成常量文本
timed 工具，恢复回放 / `inherit_prefix` / `session_has_history` 闸零改动白拿；
真机口令实验 + 跨二进制 sha256 全过（154–156、158）；157（wasm 同路）曾后置，
08-13 踩着 164（认领另一会话在飞的 wasm skills 声明落店）补做完成，真机浏览器
四钉全进（含「刷新 + 零声明宿主恢复只认 journal」）。M16 Rust 扩展包：146–149 前半（截获注册表 /
`ExtensionPack` 接缝 / 第一个真扩展包 `ext:stats` 六条真机全过）+ 150 决策（决策 30：
扩展观测「被问才算」，不建反应式层）+ 153 收口（`TimedRun` 加只读 `&Session`，
`ext:stats/audit` 轮末现读，151/152 随决策 30 撤销）；接缝见
[docs/EXTENSIONS.md](docs/EXTENSIONS.md)。每个里程碑都有真机 dogfood 收官（真 provider，
不是 mock）或全绿的三门禁验证，逐条兑现记录在 `docs/ROADMAP.md` §二和各 issue 的实做
记录里。

同一个核心库的**五种**形态都真实验收过：CLI（undo/恢复/屏障）、浏览器前端（SSE/多 agent
并行/断开取消）、独立 server bin、桌面 app（内嵌同库同前端），**以及跑在浏览器里的
wasm 宿主**（M13/M14——没有任何服务端进程，页面自己声明并执行工具）。

三条最容易过期的事实：

1. **Java 参考网关已构建验证**（OpenJDK 21 + Maven 3.9.15，037 那句「本机无 JDK」已被
   M9 推翻），M9 起它是**拉取式**——网关 poll Rust、自己产生 SSE 给浏览器，并用
   `ProcessBuilder` + `--ready-file` 拉起 Rust 子进程（[docs/INTEGRATION.md](docs/INTEGRATION.md)）。
2. **决策 10「砍掉 wasm 目标」已被 M13 推翻**——wasm 是第三种宿主形态，不替代任何一种。
3. **skills 不再是 core/runtime 的概念**（M15 决策 27，取代决策 21）：索引是一个开局
   工具、正文按需 `srv:skill/read` 以 tool result 进对话；老的激活子系统整条删掉
   （141），`Slot::SkillsActive` 只留壳给老快照反序列化。看到任何提「激活 skill」
   「`late_system` 注入」的文档，那是过期的。

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

**集成测试一律写进各 crate 的 `tests/it/`**（新建文件 + 在 `tests/it/main.rs` 加一行
`mod`），**不要在 `tests/` 顶层建 `.rs`**——顶层每个文件都是独立链接的二进制，267 个
测试文件曾两天把 target 堆到 58GB/88 万文件，2026-08-05 已合并为每 crate 一个 harness。

**但那次只修掉了一个来源。** 2026-08-13 实测 target 又到 35G/79 万文件，最大的两块
是 `incremental/`（20G）和 `deps/**/*.rcgu.o`（11.4G / 63 万文件——每个 codegen unit
一个目标文件，按构建 hash 分开存，**cargo 从不回收旧 hash 的**；`agent_cli` 一个 crate
就攒了 40 个 hash）。**定期跑 `scripts/clean-build-cache.sh`**（只清可再生的中间产物，
不动 `.rlib`，清完六道门全绿）。首次执行 35G→9G，`deps/` 文件数 63 万→3,227。
`wasm32-unknown-unknown` 目录**必须留**（`build-wasm.sh` 的产物，浏览器宿主靠它），
其余非原生目标目录是一次性交叉编译的孤儿，脚本会清掉。细节见 [issues/197](docs/issues/197-incremental-cache-bloat.md)。

## 自动检查

`scripts/check-invariants.sh` 挂在 Edit/Write 的 PostToolUse hook 上，检查能被 grep
判定的红线（行数、禁用依赖、`store.set`、`AtomId` 序列化、derived 里的时钟/随机）。
需要判断的部分（这个 atom 该 primitive 还是 derived、这个 tool 的 reversibility 等级怎么定）
走 skill `agent-state-design`。

CI 上跑同一个脚本：`scripts/check-invariants.sh --all`。
