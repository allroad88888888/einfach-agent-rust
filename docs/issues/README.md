# Issues

一个文件一个任务，每个都能被单独拿起来做。已拍板的决策在 [../ROADMAP.md](../ROADMAP.md)。

仓库还没有 remote，所以 issue 就是这些文件。做完把状态改成「完成」并补上实际结论
（**决策类**尤其：理由比结论有用）。

## 排序原则：每个里程碑都以「能用」结束

不按架构层自底向上砌墙。按层排的问题是中间没有一个点能停下来说「这东西能用了」，
而且越往后前面所有假设越是没被真实使用验证过——赌注一直在加大。

| | 结束时你能做什么 |
|---|---|
| **M1** | `cargo run` 跟模型对话，它能调工具读你的文件并回答 |
| **M2** | 按 undo 退回上一轮；退出重进能接着聊 |
| **M3** | 浏览器打开，多人用，子 agent 并行干活 |
| **M4** | 装成桌面应用；企业能内嵌进自己的服务 |

---

## M1 · 一个能用的 CLI agent

从零开始，仓库里现在只有 docs、probes、scripts。

**中途有第二个「能用了」的点：022 之后就能 `cargo run` 跟模型说上话**（没有 loop、
没有工具，直连）。这不是里程碑，是个刹车片——如果连一轮直连都跑不通，
后面那些 issue 的假设全是空中楼阁。

```
021 → 025 → 022 ─┬→ 023 → 024 ─────────────┐
                 │                          ├→ 012 ─┐
                 └→ 001 ─┬→ 002 → 016 → 003 ┘       ├→ 014（M1 终点）
                         └→ 005                     │
004 ＋ 021 → 013 ───────────────────────────────────┘
```

| # | 任务 | 依赖 | 模型 | 独测 |
|---|---|---|---|---|
| [021](021-skeleton.md) | workspace 骨架与最小值类型 | — | sonnet | — |
| [025](025-provider-seam.md) | **接缝定型**：一家对录制帧全绿（零网络） | 021 | **opus** | ✅ |
| [022](022-first-provider.md) | **打通一家 provider**（能对话） | 025 | sonnet | — |
| [023](023-three-providers.md) | 三家适配与 `Capabilities` | 022 | sonnet | ✅ |
| [024](024-cache-guard.md) | 缓存兜底三层 | 023 | **opus** | ✅ |
| [001](001-loop-contract.md) | 定 loop 的事件与 effect 契约 | 022 | **opus** | ✅ |
| [002](002-turn-state-machine.md) | turn 状态转移表 | 001 | sonnet | — |
| [016](016-stop-conditions.md) | 停止条件与取消 | 002 | sonnet | — |
| [003](003-tool-convergence.md) | 多工具收敛与部分失败 | 016 | sonnet | — |
| [012](012-wire-loop-to-transport.md) | **loop 接到真实 transport** | 003+024 | sonnet | — |
| [005](005-mock-harness.md) | 无网络测试脚手架 | 001 | sonnet | — |
| [004](004-tool-result-limit.md) | 工具结果大小上限（**决策**） | — | **opus** | 决策类 |
| [013](013-builtin-tools.md) | 内置工具：读文件 / 列目录 | 004+021 | sonnet | ✅ |
| [014](014-cli-shell.md) | **CLI 壳** ← M1 终点，扛 M1 验收 | 012+013 | sonnet | — |

**M1 验收**（可判定，不用形容词）：

- 连续十轮真实对话不出错，其中至少两轮由模型主动调工具读文件
- **第 2 轮起每一轮 `cached_tokens / prompt_tokens ≥ 0.9`**
- 十轮里三层兜底**一次都不告警**
- `Ctrl-C` 取消当前轮后进程还活着，下一轮能继续

**004 是决策类，第一天就能开工**，它不依赖任何代码。013 只需要 021 的 `ToolSpec`。

**001 依赖 022 而不是排在最前**：契约要定得对，得先知道一次真实调用长什么样。
上一版反过来了，结果 `CallProvider` 里带着组装好的请求——而组装归 adapter（决策 15）。

012 是第一次让 loop 碰真东西——**它会暴露 001–003 所有假设里错的那些**，所以单列不混进 003。

`shell/exec` 挪到 M2 的 [020](020-shell-tool.md)：它是 `Irreversible` 的，M1 没有 undo
屏障挡不住，写了也只能默认关着——那等于留一段从没跑过的代码。

## M2 · 能撤销、能续上

| # | 任务 | 依赖 | 模型 | 独测 |
|---|---|---|---|---|
| [007](007-fork-store.md) | fork 并去 Excel 化（到编译通过为止） | M1 | haiku | — |
| [015](015-port-store-tests.md) | 移植上游的行为测试 | 007 | haiku | — |
| [008](008-split-store.md) | 按职责拆分 `store.rs` | 015 | sonnet | — |
| [009](009-history.md) | `Entry` 结构与写入记录 | 008 | **opus** | ✅ |
| [017](017-undo-redo.md) | undo/redo 的两层粒度 | 009 | **opus** | ✅ |
| [018](018-history-cap.md) | 日志上限与分支覆盖 | 017 | sonnet | — |
| [019](019-applier-recreate.md) | 已 evict atom 的按需重建 | 017 | **opus** | ✅ |
| [010](010-snapshot.md) | `snapshot.rs`：快照与恢复 | 019 | **opus** | ✅ |
| [020](020-shell-tool.md) | `shell/exec`：第一个 `Irreversible` 工具 | 017 | sonnet | ✅ |
| [011](011-session-store.md) | `SessionStore` 端口 + Memory/Jsonl | 010 | sonnet | — |
| [026](026-state-into-atoms.md) | **状态搬进原子图（command 层）** ← M2 最重 | 019 | **opus** | ✅ |
| [027](027-cli-undo.md) | CLI `/undo` `/redo` + 恢复 ← M2 终点 | 026+011 | sonnet | ✅ |

**M2 验收**：CLI 里 undo 退回上一轮且所有派生值一致；undo 越过 `shell/exec` 时停下
推 `undo_blocked`；杀掉进程重启后会话还在。

007 / 015 / 008 刻意分成三个——移植、验证行为、拆分混在一起做，出问题时分不清是
移植错了、行为变了、还是拆错了。

026/027 是 M1 跑完后按实际形状细化的（019 的三条硬约束、017 推的 epoch 账、
020 攒的屏障开关全部是它们的设计输入——先做的 issue 用血换的教训都在里面）。

## M3 · 远程与多 agent

[006](006-subagent-spawn.md) 已拍板（决策 20：模型经工具 spawn）。两条链：

```
链 A（多 agent） 028 → 029 ─────────────┐
链 B（远程）     030 → 031 → 032 → 033 ─┴→ M3 验收
```

| # | 任务 | 依赖 | 模型 | 独测 |
|---|---|---|---|---|
| [028](028-multi-agent-graph.md) | **多 agent 原子图**：AgentId 路径语义 + 上下读边界 + despawn | 026 | **opus** | ✅ |
| 029 | `spawn_agent` 工具 + 硬限 + runner 子树驱动 | 028 | **opus** | ✅ |
| 030 | session actor：mpsc 进 / broadcast 出，store 独占线程 | 026 | sonnet | — |
| 031 | `agent-server` 库：六端点 + SSE 补发 + 断开取消在飞 | 030 | sonnet | ✅ |
| 032 | `packages/protocol`：TS 类型从 Rust 生成 | 031 | sonnet | — |
| 033 | web 最小客户端 ← M3 终点 | 031+032 | sonnet | — |

**M3 验收**：真浏览器连 SSE 拿到流；断开能取消在飞请求（不白烧 token）；
一个任务真的被模型分解给子 agent 并行、undo 一轮连带子树回滚。

029–033 的 issue 文件随链条推进逐个写（026/027 的先例：晚写的 issue 吃到
先做 issue 的全部教训）。

## M4 · 装得上、嵌得进

| # | 任务 | 依赖 | 模型 | 独测 |
|---|---|---|---|---|
| [035](035-server-bin.md) | `agent-server-bin`：二十行宿主 | — | sonnet | — |
| [036](036-tauri-desktop.md) | Tauri 桌面内嵌（含 server 静态托管选项）← M4 终点 | 035 | sonnet | — |
| [037](037-java-gateway.md) | Java WebFlux 参考网关（本机无 JDK，写好+文档，构建验证如实标注） | — | sonnet | — |

**M4 验收**：桌面 app 打开即同一套 web 前端、内嵌同一个 server 库、真实对话可用；
`agent-server-bin` 独立起、Java 网关文档能指导企业内嵌。

## M5 · skills 装载

| # | 任务 | 依赖 | 模型 | 独测 |
|---|---|---|---|---|
| [038](038-skill-injection-probe.md) | **探针**：消息级 system 注入的三家实测 | — | sonnet | 探针类 |
| [039](039-skills-loading.md) | skills 装载全链（registry/激活工具/注入/undo） | 038 | **opus** | ✅ |

**M5 验收**：放一个 skill 进目录 → 模型在对话里自己发现并激活它 → 用上它带的
工具 → `/undo` 连激活一起退掉；中途激活在 DeepSeek 上**不炸前缀**（探针结论
兑现到对账数字）。

未排期：**skills 装载**（system 段注入与 late_tools 的来源——料单字段已留位，M1 用完两周再定形态）、`agent-mcp`、多租户、多副本的 `RedisRegistry`。

---

## 怎么做

粒度标准、每个 issue 用什么模型、测试由谁写，见 **[../WORKFLOW.md](../WORKFLOW.md)**。

一句话版本：**按「错了能不能立刻发现」选模型**——编译不过用 haiku，测试会红用 sonnet，
**不报错只在 undo / 崩溃恢复 / 账单上浮出来的用 opus 并派独立 agent 写测试**。
第三档不用凭感觉判断，看这个 issue 的「注意」有没有提到红线 1–6、11 或 12。

## 约定

**决策类 issue** 动手前必须先定，因为它们改变后续代码的形状。当前只有
[004](004-tool-result-limit.md) 在 M1 的关键路径上；
[006](006-subagent-spawn.md) 已挪到 M3——单 agent 的 CLI 已经有用，在没有真实使用
反馈的情况下定它等于猜。

每个 issue 的「注意」一节列了它会踩到的红线。动手前顺手看一眼
[../INVARIANTS.md](../INVARIANTS.md)：红线 1–6、11、12 违反后**不报错**，
只在 undo、崩溃恢复、账单或「加第四家 provider」时浮出来。

模型适配层的接缝定义在 [../ADAPTER.md](../ADAPTER.md)，022/023/024 动手前必读。
