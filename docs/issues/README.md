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
| [029](029-spawn-tool.md) | `spawn_agent` 工具 + 硬限 + runner 子树驱动 | 028 | **opus** | ✅ |
| [030](030-session-actor.md) | session actor：mpsc 进 / broadcast 出，store 独占线程 | 026 | sonnet | — |
| [031](031-http-sse.md) | `agent-server` 库：六端点 + SSE 补发 + 断开取消在飞 | 030 | sonnet | ✅ |
| [032](032-protocol-gen.md) | `packages/protocol`：TS 类型从 Rust 生成 | 031 | sonnet | — |
| [033](033-web-client.md) | web 最小客户端 | 031+032 | sonnet | — |
| [034](034-server-multiagent.md) | server 接满多 agent ← M3 终点 | 033 | sonnet | — |

**M3 验收**：真浏览器连 SSE 拿到流；断开能取消在飞请求（不白烧 token）；
一个任务真的被模型分解给子 agent 并行、undo 一轮连带子树回滚。

029–034 的 issue 文件随链条推进逐个写（026/027 的先例：晚写的 issue 吃到
先做 issue 的全部教训）——**都已写完并落地**，上表的链接是事后补的。

## M4 · 装得上、嵌得进

| # | 任务 | 依赖 | 模型 | 独测 |
|---|---|---|---|---|
| [035](035-server-bin.md) | `agent-server-bin`：二十行宿主 | — | sonnet | — |
| [036](036-tauri-desktop.md) | Tauri 桌面内嵌（含 server 静态托管选项）← M4 终点 | 035 | sonnet | — |
| [037](037-java-gateway.md) | Java WebFlux 参考网关（当时本机无 JDK，只写好+文档；**M9 已真构建验证并推翻这句**，见 058） | — | sonnet | — |

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

## M6 · MCP 接入 ✅（真机 dogfood 收官 2026-08-03）

原始蓝图最后一块。接缝定义在 [../MCP.md](../MCP.md)（决策 22）。MCP 当 adapter 接——
和 provider 同类接缝，只是要做 IO。照 022「先打通一家」的先例，MVP = **stdio + tools**。

```
040(决策) → 041(协议+翻译,零IO) → 042(stdio+握手,真子进程)
                                      → 043(执行路由+epoch,红线6) → 044(配置+失败隔离) → 045(CLI终点)
```

| # | 任务 | 依赖 | 模型 | 独测 |
|---|---|---|---|---|
| [040](040-mcp-seam.md) | **MCP 接缝定义**（决策：crate 边界/可逆性元数据/异步执行/范围） | — | **opus** | 决策类 |
| [041](041-mcp-protocol.md) | 协议类型 + JSON-RPC 帧 + 翻译（零 IO，录制帧全绿） | 040 | sonnet | ✅ |
| [042](042-mcp-stdio.md) | stdio 传输 + 握手（真子进程，句柄住 store 外） | 041 | sonnet | — |
| [043](043-mcp-execution.md) | 执行路由 + 可逆性元数据 + epoch 回写（红线 6） | 042 | **opus** | ✅ |
| [044](044-mcp-config.md) | `.mcp.json` 装载 + 多 server + 失败隔离 | 043 | sonnet | — |
| [045](045-mcp-cli.md) | CLI 接入 + `/mcp` 状态 ← M6 终点 | 044 | sonnet | — |

**M6 验收 ✅ 兑现**（主会话真机 dogfood，deepseek + npx `server-everything` 13 工具）：`/mcp`
列 everything=connected；模型自发 `mcp:everything/echo`（`reversibility=Pure`，从 `readOnlyHint`
翻译）→ 拿真结果 `Echo: ...` → 用它回答；第 2 轮缓存 `预测=实际 7040`（红线 11）；`/undo`
干净越过 Pure 调用；kill-9 全新进程 `会话已恢复` + MCP 从 `.mcp.json` 重连；无孤儿 npx。
六个 issue 全绿，详见 [045 实做记录](045-mcp-cli.md) 的主会话 dogfood 段。

未排期（M6 明确延后，等真实反馈）：**http/sse 远端传输**（浏览器 host 的 MCP）、
**resources → skill 资产**、**prompts → skills**、**OAuth**、多租户、多副本的 `RedisRegistry`。

## M7 · 子 agent 可观测（插在 M6 中间）

「子 agent 不该是黑盒，界面要显示它在干啥」——真实使用反馈驱动。接缝定义见
[../OBSERVABILITY.md](../OBSERVABILITY.md)（决策 23）。核心：可观测性是对现有 atom 的一次
**派生读**，不新增 primitive、不让 UI 重建状态机。

```
046(接缝+core派生读) → 047(CLI /agents,最小能用) → 048(SSE快照事件+GET) → 049(web树面板,终点)
```

| # | 任务 | 依赖 | 模型 | 独测 |
|---|---|---|---|---|
| [046](046-agent-tree.md) | 接缝 + `agent_tree()` 派生读 + `AgentTree`/`AgentNode`（core，ts-export） | 028 | sonnet | ✅ |
| [047](047-cli-agents.md) | CLI `/agents` 文本树 ← 最小「能用」刹车片 | 046 | sonnet | — |
| [048](048-tree-sse.md) | SSE 快照变化事件 + GET 端点（协议一致性 + Last-Event-ID） | 046 | sonnet | ✅ |
| [049](049-web-tree.md) | web / 桌面活树面板 ← M7 终点 | 048 | sonnet | — |
| [050](050-tool-name-encoding.md) | **真机 dogfood 捞到**：工具名 URL 编码泄漏进模型的 tool-name 参数（`tool_name::resolve` 归一化，模型自纠有效、非阻塞） | — | **opus** | ✅ |

**M7 验收**：真浏览器里，模型 spawn 子 agent → 活树面板实时长出节点、状态灯随
`Thinking`→`ToolsPending`→终态变、activity 显示当前动作（含 043 后的 MCP 在飞调用）；
`/undo` 撤一轮 → 被撤子 agent 从树上消失；断开重连 → 树恢复正确。

范围 = **活树**（当前快照 + 变化推送）。**可回放时间线**（任意 epoch 的树快照）延后——
活树是它的当前 epoch 特例，等真需要回溯审计再加。

**049 前端件已交**：树面板（`render/agent_tree.ts`）+ SSE 接线（`dispatch.ts`）+
`GET /sessions/:id/agents` 做种（`api.ts`/`main.ts`）+ 布局样式全部落地，
`pnpm --filter web typecheck`/`build` 前台跑绿，详见 [049 实做记录](049-web-tree.md)。
**四条真机验收断言**（长出节点/状态灯变/undo 回退/reconnect 恢复）需要真浏览器 + 真
provider，留给主会话跑，本次不算数（issue 头「独测 —（终点靠真浏览器验收）」的既定
安排）。

## M8 · 模型侧异步编排（设计完成，实现排 M6 收官后）

M7 给**人看**的活树；用户追问「模型自己要不要工具去获取子 agent」——**模型看**的对偶。选
大版本：三个工具让模型**中途观测子 agent 并改变编排**（不是加并行——并行 spawn 早有）。
接缝见 [../ORCHESTRATION.md](../ORCHESTRATION.md)（决策 24）。**关键决策：子 agent 仍不跨
turn**（turn 内异步），避开跨-`run_turn` 映射/undo 重写/per-child 取消，红线全不破，决策 20
前台 spawn 一行不改。

```
051(status,独立可先发) → 052(spawn background+孤儿取消) → 053(collect) → 054(面板+真机dogfood,终点)
```

| # | 任务 | 依赖 | 模型 | 独测 |
|---|---|---|---|---|
| [051](051-agent-status-tool.md) ✅ | `srv:agent/status` 非阻塞下读子树（纯观测半边，独立可先发） | 046 | sonnet | ✅（红线 11） |
| [052](052-spawn-background.md) ✅ | `spawn(background)` + detached 名单 + 孤儿收尾 | 043 后 | opus | ✅（红线 6 + pump 不变量） |
| [053](053-agent-collect-tool.md) ✅ | `srv:agent/collect` 领后台子结果（复用 harvest 回写） | 052 | opus | ✅（红线 6） |
| [054](054-orchestration-surface-dogfood.md) ✅ | 面板呈现 bg/collect + 真机 dogfood ← M8 终点 | 051/052/053 | sonnet | 真机 |

**M8 验收 ✅ 兑现**（主会话真机 dogfood，deepseek 真实上游 + curl 直打，不经浏览器）：模型自发
`spawn(background)`×3 → `status` 观测 → `collect`×3 → 汇总；树快照第 141 帧
**`root:Thinking` + 三个子同时 `Thinking`**——这一帧在 M7 的阻塞 spawn 下**结构上不可能**
（那时 root 卡在 `ToolsPending` 直到子收敛），是 M8 的全部增量；含后台子的 turn `/undo` →
21 条 entry 连带整棵子树退干净、树只剩 `root:Idle`。详见
[054 真机 dogfood 段](054-orchestration-surface-dogfood.md)。

**051 已落地**（2026-08-04）：`srv:agent/status` 三件事齐（`status_tool.rs` 声明+收窄+渲染、
`ToolTable::with_status()`、dispatch 纯读截获），接进 `agent-cli` 与 `ToolTableSpec::Full`。
收窄住宿主不住 core、红线 11 自己排序，详见 [051 实做记录](051-agent-status-tool.md)。
它不依赖 052/053，可单独用。

**M8 验收**：真机（deepseek 真实上游）上模型自发 `spawn(background)` × N → `status` 观测谁快
谁慢 → `collect` 领结果；活树面板**同时**显示多个后台子在 Working（M7 时子是阻塞串行的，这是
相对 M7 的新现象）；含后台子的 turn `/undo` → 树回退。

延后（等真实反馈）：**跨 turn 后台 agent**（子活过一次 `run_turn`，像 shell 的
`run_in_background`）、**per-child cancel 工具**（单杀一个后台子）——见 ORCHESTRATION §六。

## M9 · 企业集成（拉取式传输 / chatid 身份 / 进程生命周期）

真实提问驱动。接缝定义见 [../INTEGRATION.md](../INTEGRATION.md)（决策 25）。核心判断：
**SSE 的复杂度只该出现在「产生 SSE」那一跳（Java→浏览器），不该出现在「代理 SSE」那一跳**
——那四个坑（不缓冲/不压缩/超时/取消传播）+ 强制 WebFlux 全在代理这一跳，而企业存量多是 MVC。

```
059(hub泄漏,先修) → 055(chatid幂等) ─┐
                    056(拉取端点) → 057(断开检测) ─┴→ 058(Java网关+真机, 终点)
```

| # | 任务 | 依赖 | 模型 | 独测 |
|---|---|---|---|---|
| [059](059-hub-leak.md) ✅ | **hub 表永不回收**（自持有导致 `remove` 执行不到）← 排最前 | — | **opus** | ✅ |
| [055](055-chatid-session.md) ✅ | `chatid` 幂等 getOrCreate + id 白名单（拒绝不 sanitize） | 059 | sonnet | ✅ |
| [056](056-poll-endpoint.md) ✅ | 拉取式端点 `GET /events/poll`（ring 的第二个投影） | 059 | sonnet | ✅ |
| [057](057-poll-disconnect.md) ✅ | 拉取式断开检测（每次 poll 持 `SubscriberGuard`） | 056 | **opus** | ✅ |
| [058](058-java-gateway-pull.md) ✅ | Java 网关：拉取 Rust → 产生 SSE + 进程生命周期 ← M9 终点 | 055+057 | sonnet | 真机 |

**M9 验收 ✅ 兑现**（主会话真机全链，OpenJDK 21 + 真 deepseek + curl，全程没手工起 Rust）：
Java 网关 `ProcessBuilder` 拉起 Rust 子进程、经 **ready-file** 拿到 OS 分配的端口（`--port 0`
→ 49611，不解析 stderr）；`POST /agent/sessions {"id":"gw-chat-1"}` → 201 `created`、再来
一次 → 200 `existing`；`GET /agent/sessions/{chatid}/events` 拿到 **67 帧**、**`id:` 游标保留**、
`thinking_delta` **逐帧到达不缓冲**（「产生 SSE」而非「代理 SSE」，那四个坑结构上不存在）；
停 Java → **Rust 一起干净退出、无孤儿**、会话落盘；全新进程同 chatid → 200 `recovered`。
详见 [058 真机全链段](058-java-gateway-pull.md)。

**059 已落地**（2026-08-04）：**实测确认真的漏**（三个 close 完的 session，五秒后 hub 表
一项没少）。修法是 `SseHub` 只留 `CancelHandle`（取消那一半，不含 `events` 发送端），
drain 任务从此等得到 `None`、摘表退出；宽限取消一步没改。详见
[059 实做记录](059-hub-leak.md)。

**055 已落地**（2026-08-04）：`POST /sessions` 收下可选 `id`，三态靠「registry 查一次 +
`open` **之前**看一眼默认 jsonl 在不在」判定（200 `existing` / 200 `recovered` / 201
`created`），恢复仍走 kill -9 那条既有路，没造新机制。白名单只在**收下 id 的这一处**，
不合规 400 且不留任何文件系统痕迹（沙箱整棵树逐项断言，反向验证过 `../../etc/passwd`
真的会写出去）。不给 `id` 的旧调用方响应形状一字未变。详见
[055 实做记录](055-chatid-session.md)。

**058 代码部分已落地**（2026-08-04）：Rust bin 加 `--ready-file`（bind 成功后原子发布
`{"port","pid","version"}`，`hard_link` 而非 `rename`——陈旧文件不可能被当成本次成功）；
网关删掉代理上游 SSE 那条路，改成 25s 长轮询 + 自己产生浏览器 SSE，`next` 直接当下次
`Last-Event-ID`；chatid 走 055 幂等三态；`ProcessBuilder` + `@PreDestroy` 持有子进程生死；
最后一条浏览器连接走掉时发显式 `POST /cancel`（宽限降为兜底）。`mvn -q package` 已用
OpenJDK 21 + Maven 3.9.15 **真跑通过**，037 那条「本机无 JDK」的声明随之作废（但「没有
JDK 时不许伪造构建结果」的规矩保留）。详见 [058 实做记录](058-java-gateway-pull.md)。
**真机全链 dogfood 仍待主会话**，所以这一行还没打 ✅。

**M9 验收**：Java 网关自己拉起 Rust 子进程 → 浏览器带 chatid 打开、真实对话可用 →
同 chatid 重开页面历史还在 → 关掉浏览器宽限后在飞轮次被取消（不白烧 token）→
停掉 Java 进程 Rust 子进程一起干净退出、`ps` 无残留。

**两条安全线**（写进 058 的 README，代码解决不了的部分）：chatid 拼进文件名 →
**白名单拒绝、不 sanitize**（悄悄改写会让两个 chatid 撞进同一个会话文件）；chatid 即身份 →
**归属由网关保证**（猜到别人的 chatid 就能接上别人的会话）。

**否决**：JNI / Panama FFI 真嵌入——流式跨 FFI 难做好、Rust panic 跨边界杀 JVM、进程隔离
全丢；决策 12 说的「企业内嵌 `agent-server` 库」指的是**企业自己写 Rust 服务**去内嵌。
**延后**：网关侧共享拉取（多观众只拉一份）、WebSocket、多副本粘性路由。

## M10 · 宿主能力注入（前端/网关声明自己的 tool、skill、MCP）

「tool skills mcp 从前端一开始注入」——真实需求驱动。接缝见
[../HOST-CAPABILITIES.md](../HOST-CAPABILITIES.md)。**核心判断：不需要新机制，只缺一个声明
入口**——执行通道（remote tool）与延迟加载（skill 索引+激活）都已完整存在且测过，
宿主注入的能力跟自有的走**完全相同**的路。

```
060(远端挂死,前置)
  ├─ Rust 线： 061(协议+校验) → 062(per-session装配) → 064(skill注入+唤醒)
  │                              └∥ 063(红线11确定性锁，与062并行)
  └─ 前端线： 065(注入声明) → 066(执行remote tool) → 067(MCP客户端)
                                                        └→ 068(真机, M10终点)
```

| # | 任务 | 依赖 | 模型 | 独测 |
|---|---|---|---|---|
| [060](060-remote-tool-hang.md) | **远端工具两个挂死面**（未声明的 `web:` 进等待槽 / 等待无超时）← 前置 | — | **opus** | ✅ |
| [061](061-capabilities-protocol.md) | `capabilities` 协议类型 + 名字校验（**纯数据零 IO**） | — | sonnet | — |
| [062](062-capabilities-assembly.md) | per-session 装配：注入工具进这个会话的表 + 可逆性映射 | 061 | **opus** | ✅ |
| [063](063-capabilities-determinism.md) | **红线 11 字节确定性锁**（独测，与 062 **并行**） | 061 | **opus** | ✅ 本体 |
| [064](064-capabilities-skills.md) | `capabilities.skills` + **唤醒 server 形态的 skill 机制** | 062 | sonnet | ✅ |
| [065](065-frontend-inject.md) | 前端：建会话时注入 capabilities | 061 | sonnet | — |
| [066](066-frontend-tool-exec.md) | 前端：执行 remote tool 并回传 | 060 | sonnet | ✅ |
| [067](067-frontend-mcp-client.md) | 前端 MCP 客户端（**形态 B**：浏览器自己连） | 065 | sonnet | — |
| [068](068-host-capabilities-dogfood.md) | 真机 dogfood ← M10 终点 | 064+066+067 | 主会话 | 真机 |
| [073](073-capabilities-into-store.md) | **声明进 store：恢复时原模原样复刻**（用户拍板；不是重新注入） | 062 | **opus** | ✅ |

**MCP 的关键分岔**（HOST-CAPABILITIES §七）：**否决**「前端交配置、server 去连」——那是把
RCE（`command` 任意执行）和 SSRF 写进协议，**在任何安全策略下都不该存在**；**采用**
「前端自己连 MCP，把**工具**注入进来」——server 完全不碰 MCP 协议、不 spawn 任何东西，
执行走既有 remote 通道，**零新机制**。这恰好补上 M6 延后的「浏览器 host 的 MCP」。
命名 `web:mcp-<server>/<tool>`，与服务端 MCP（`mcp:everything/echo`）**同会话共存不冲突**。

**并行与撞车**：Rust 线与前端线**并行**，组内串行。`api.ts` 由 065（只加 `createSession`
参数）与 066（只加 `sendToolResult`）分工，067 以新建 `src/mcp/` 为主。

~~066 与 038-frontend-tools（另一会话在做）高度重叠，先看它交了什么~~——**已作废**：
那个「前端工具闭环」文件是一次崩掉的写入留下的 124 字节残骸（标题写到一半就断了），
而且**撞了 038 这个已经被探针占着的号**。066 自己做完了这件事（`packages/web/src/tool-exec.ts`
+ 23 条测试），残骸已删。留这条记录是因为「指向一个只有标题的文件」比没有指路更费时间。

**两个勘查发现**：①`docs/TOOLS.md` 画的 `ToolDescriptor`（带 `location`/`reversibility`/
`source`）**代码里不存在**——实际 `ToolSpec` 只有三字段，位置和可逆性靠**不查表的自由函数
按名字推**，应顺手修正文档；②**server 形态下 skill 从没被装载过**（五档都不接
`.with_skills`，`SkillRegistry::load` 只有 CLI 调）——**064 已唤醒**：宿主声明的 skill 进
per-session registry，registry 非空才接 `.with_skills(..)`；**server 不从磁盘 `./skills/`
装载**（069 §拍板，理由见 064 实做记录）。

**安全**（HOST-CAPABILITIES §九）暂缓讨论，定稿后可能追加 issue。

### M10 期间捞到的独立 issue（都不在原计划里，真实发现驱动）

| # | 任务 | 来源 | 模型 | 状态 |
|---|---|---|---|---|
| [069](069-name-collision-policy.md) ✅ | **多来源撞名的冲突策略**（拍板：不统一行为，统一成一条红线「撞名不许留到 prompt 里」+ 一条判据「在最早能报给作者的点上失败」；四条路的差异各有依据。工具表的实现排在 062 之后，本次落看门狗测试） | 文档审计 D8 | **opus** | ✅ 拍板 |
| [070](070-mcp-registry-global-lock.md) ✅ | **MCP 调用被一把全局锁串行化**（实测坐实：fast 被 slow 全额挡住 1.011s；修法 `Arc<Mutex<McpClient>>` 两层锁，同 server 仍串行） | 文档审计 | **opus** | ✅ |
| [071](071-status-tool-description-lies.md) ✅ | 工具**说明书对后台子 agent 说假话**（审计只捞到 status；核对发现 `spawn` 更糟——「需要答案就别开后台」在 053 后是**反的**。测试用关键子串+工具名常量） | 文档审计 | sonnet | ✅ |
| [072](072-replay-reexecutes-tools.md) | **重连/刷新让前端把历史里的工具调用再执行一遍**（副作用真发生第二次；示例工具是 `pure` 故暂时无害，注入业务工具后就是重复下单）。**前提已勘误**：demo 刷新是**新会话**（`api.ts:51` 没有 chatid 参数），复现要的是「复用 chatid + 无游标的新客户端」= **M9 网关/多 tab 那条路**，不是浏览器刷新。**已拍板**：待办投影——`GET /pending_tools` 导出还欠着的等待槽，**帧只是触发器、服务端状态才是判据**；「是不是补发」判据本身就错（会漏活），且拉取式下每帧都来自 ring、这个区分在正主路径上根本不存在。`Frame` 不动 | 066 落地时发现 | **opus** | ✅ 拍板，待实现（**068 真机前必须有答案**） |
| [074](074-mcp-list-tools-duplicate-names.md) ✅ | 同一个 MCP server 的 `tools/list` 回包里**两项同名，整条链没人拦**——`specs` 是 `Vec` 两条都进 prompt，可逆性走 `BTreeMap::insert` 后来居上，模型看第一份说明书、undo 屏障用第二份的可逆性。修法：`McpClient::list_tools` 这一跳按名字去重（保留第一条，丢后来的整条），告警（server id + 重复的名字 + 丢了几条）经 `LoadOutcome.warnings` 送到边界，不碰 `Availability`/`ServerStatus` 的既有形状 | 069 复查时捞到 | sonnet | ✅ |
| [075](075-tool-table-drops-duplicate-names.md) | **工具表自己不判重**（074 的兜底那一层）：同一批数据里 spec 走 `Vec::push` 两条都留、可逆性走 `BTreeMap::insert` 后来居上。修法：私有 `push_spec`，重名整条丢弃（spec 与可逆性一起跳过）+ `debug_assert!` 点名。069 已证实**当前装配链没有实际撞名**，所以它是护栏不是修 bug | 069 拍板的代码清单 | sonnet | ✅ |
| [076](076-per-session-builtin-switches.md) | **建会话时挑选内置能力**：`capabilities` 加一个**减法**字段，关掉的工具连名字都不进 prompt。两条不商量的约束：**只能减不能加**（客户端不许突破部署方的天花板）+ **开关进 store**（073 那条规矩，否则恢复出来跟当初不一样）。子 agent 不单独配，整棵树共用 | **用户提出** 2026-08-04 | **opus** | ✅ |
| [077](077-flaky-under-load.md) | **测试套件在高负载下假红**：定性结论是**测试写得不严，不是运行时重发**。三份假上游把 listener 设了非阻塞，而 BSD/macOS 上 accept 出来的 socket **继承** O_NONBLOCK——请求字节晚到一瞬就被当成「没带请求」，于是**多记一次假请求 + 错位脚本 + 把客户端那次真调用弄坏**。`request_count()` 数的是连接不是请求。修计数口径与构造，`== 1` 那条断言一个字没动 | 076 收工代收 | **opus** | ✅ |

另有一份 [DOC-AUDIT.md](../DOC-AUDIT.md)（文档↔实现一致性审计：危险 10 / 过时 40 /
小瑕疵 19 / 疑似代码问题 4）。TOOLS/STATE-MODEL/ARCHITECTURE/CLAUDE.md 的修正已落地，
报告保留作为对照底本。

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
