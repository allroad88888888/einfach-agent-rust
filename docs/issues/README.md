# Issues

一个文件一个任务，每个都能被单独拿起来做。已拍板的决策在 [../ROADMAP.md](../ROADMAP.md)。

issue 就是这些文件（M12 期间仓库已有 remote：`github.com/allroad88888888/einfach-agent-rust`，
但任务仍然只活在这些文件里，没有用 GitHub issue）。做完把状态改成「完成」并补上实际结论
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
| [077](077-flaky-under-load.md) ✅ | **测试套件在高负载下假红**（20 轮里 10 轮红、15 条测试，一个病十几副面孔）。根因：BSD/macOS 上 `accept()` 出来的 socket **继承** listener 的 `O_NONBLOCK`，`WouldBlock` 被当成 EOF → 空请求照样记账 + 吃掉脚本槽位 + 弄坏客户端那次真调用。定性：**测试写得不严，不是运行时重发**（第二次请求是有声的合法重试 `Notice::Retrying`，无静默双倍计费）。修后 20/20 全绿，断言一个字没动 | 076 代收时发现 | **opus** | ✅ |
| [078](078-server-form-mcp-is-dormant.md) | **server 形态下 MCP 是休眠的**：`with_mcp` 在 `agent-server` 里一次都没被调用，只有 CLI 装 MCP——经 HTTP 起的会话里没有任何 `mcp:` 工具。跟 064 的「skill 休眠」同形状；挡住 068 第四条的后半句（两条 MCP 路共存） | 068 真机时撞上 | sonnet | |
| [079](079-image-content-block.md) ✅ | **`ContentBlock::Image` 变体**（`reference`/`mime`/`name`，`reference` 对 core 完全不透明，同 `ToolCallId`）。四处落点已查明并写进 issue：**只有 `wire/messages.rs:62` 会编译报错**，另三处是 `_ =>` 兜底、对图片恰好正确 | **用户提出** 2026-08-04 | **haiku** | ✅ |
| [080](080-adjustment-images-dropped.md) ✅ | **`Adjustment::ImagesDropped` 变体**（只加类型，不写触发逻辑）+ ts 导出一致性 + 前端 `switch` 补分支 | 同上 | **haiku** | ✅ |
| [081](081-image-user-input.md) ✅ | **用户输入带图**：`Event::UserInput` 带图片块，**块顺序定死**（文本在前、图片按宿主给的顺序在后，红线 11）；undo/redo/落盘逐字段复原 | 同上 | sonnet | ✅ |
| [082](082-image-array-encoding.md) ✅ | **wire 的数组编码机制**：有图才用数组、**无图逐字节不变**（现有 golden 一个都不该改）；「吃不吃图」由调用方传进来，不许在共用文件里 `match provider` | 同上 | sonnet | ✅ |
| [083](083-image-provider-fallback.md) ✅ | **三家接线与降级告警**：实测 Kimi ✓ / DeepSeek ✗ / GLM ✗；吃不下的编成占位文本**并报 `ImagesDropped`**——**静默丢图是 M11 唯一用户永远发现不了的失败** | 同上 | sonnet | ✅ |
| [084](084-transport-files-upload.md) ✅ | **transport 的图片上传**：`POST /files`（`purpose:"image"`）→ 拼 `ms://<id>`；超限在发之前拦下；假 server 记得 `set_nonblocking(false)`（077 的坑） | 同上 | sonnet | ✅ |
| [085](085-http-image-ingress.md) ✅ | **HTTP 上行**：`InputRequest` 加图片字段（不带图逐字节不变）；**先上传成功再 dispatch**，失败 400 且 store 不留残骸；**上传绝不能放进 `provider_call::start`**（会把多 agent 并行掐死，且不报错） | 同上 | sonnet | ✅ |
| [086](086-image-frontend.md) ✅ | **前端选图 / 粘贴 / 拖拽**：三条入口（粘贴最常用）、缩略图可删、`revokeObjectURL`、非图片当场拦下 | 同上 | **haiku** | ✅ |
| [087](087-image-dogfood.md) ✅ | **图片真机 dogfood ← M11 终点**：真浏览器 + 真 Kimi，六条——模型答出图里埋的四位数、第 2 轮缓存对账、undo/redo 后仍看得见、**DeepSeek/GLM 上降级告警可见**、不选图老路不变、多张图 | 同上 | 主会话真机 | ✅ |
| [088](088-kimi-upload-endpoint.md) ✅ | **Kimi 上传端点不能从聊天 endpoint 推导**：显式传递上传 API base，使 `/files` 不再追加到 `/chat/completions` | 087 真机发现 | 主会话真机 | ✅ |
| [089](089-kimi-image-cache-accounting.md) ✅ | **Kimi 图片历史的缓存预测差**：历史 `ms://` 图片以实测视觉 cache block 对账，真实第 2 轮为 `predicted=1834`、`actual=1834` | 087 真机发现 | 主会话真机 | ✅ |
| [090](090-image-undo-timeline.md) ✅ | **图片卡片未随 undo/redo 还原**：server history 已恢复，浏览器时间线却在 undo 后仍留图 | 087 真机发现 | sonnet | ✅ |
| [091](091-nonvisual-image-ingress.md) ✅ | **非视觉 provider 在 adapter 降级前被 HTTP 上传短路**：必须使 `ImagesDropped` 能实际抵达用户 | 087 真机发现 | **opus** | ✅ |
| [092](092-remote-tool-result-protocol.md) | **远端工具认领、终态回执与结果协议**：claim 后执行，稳定 submission 幂等重投，区分未认领超时与结果未知 | 用户提出 | **gpt-5.6-sol / xhigh** | 协议/Java 透传/100 轮压测 ✅；双端真机待验 |
| [093](093-vision-subagent-delegation.md) ⚠️ | **非视觉 agent 委派视觉子 agent**：DeepSeek root → 窄范围 Kimi 子 agent 检查用户图片（旧 vision 委托管线） | — | **gpt-5.6-sol** | ⚠️ 已被 s5 取代 |
| [094](094-structured-operational-logging.md) ✅ | **结构化操作日志**：一条 tracing 管线贯穿 server/bin/desktop，请求级 span + 安全生命周期事件，`RUST_LOG` 控制过滤，JSON 可切 | — | **gpt-5.6-sol** | ✅ complete |

> ⚠️ **079–091、093 的 images/vision 管线已被 s5 重构取代**：`ContentBlock::Image` / `POST /files` /
> `upload_base_url` / `ImagesDropped` / 前端选图 / 视觉子 agent 委托这条管线已整体移除，现以 `POST /uploads`
> 上传端点 + `srv:vision/inspect` 工具取代。上述 14 条仅作历史决策档案保留，不再反映当前实现。

另有一份 [DOC-AUDIT.md](../DOC-AUDIT.md)（文档↔实现一致性审计：危险 10 / 过时 40 /
小瑕疵 19 / 疑似代码问题 4）。TOOLS/STATE-MODEL/ARCHITECTURE/CLAUDE.md 的修正已落地，
报告保留作为对照底本。

## M12 · 上下文压缩（五档分级，发送侧裁剪）

**核心判断：完整对话记录一律入库、永不压缩，压缩只改「这一轮发什么」。**
这一条拆掉了决策 18 底下那个没写出来的假设（压缩会替换历史），于是 ROADMAP §四
那条「压缩与 undo 的窗口对立（P3）」不是被折中，是**不成立了**——压缩 entry 的
`prev` 只有边界值与引用，cap 100 条随便吃。决策 18 的三分原样保留。

五档按「丢失的不可逆程度」排：**1 进门截断**（已有）、**2 清工具返回**、**3 摘要**、
**4 清窗口**、**5 子 agent**（已有）。只有 3 需要模型调用、不可重算，所以它最后开火。

**动手前必读 [095](095-compaction-tiers.md) 与 [096](096-compaction-trigger.md)**
——形状与时机都在那里定死，别在实现 issue 里重新讨论。

```
两个决策根（都要在主干之前）
095(形状) → 096(时机)

可并行的独立根
097(核查取料)   098(单轮超窗兜底,决策)

主干（等 095+096）
099 → 100 ─┬─ B 支：101 → 102 → 103 ─┐
           └─ C 支：104 → 105 → 106 → 107 ─┴→ 108 → 109 → 110（M12 终点）
```

B 支与 C 支从 100 之后**完全并行**，两个 agent 互不碰对方的文件。

| # | 任务 | 依赖 | 模型 | 独测 |
|---|---|---|---|---|
| [095](095-compaction-tiers.md) ✅ | **决策**：五档分级 + 存/发分界 + `SendPlan` 形状 + undo 语义 —— **压缩不设 undo 屏障**（完整记录没被动过） | — | **opus** | 决策类 |
| [096](096-compaction-trigger.md) ✅ | **决策**：触发策略 —— **85% 触发、压到 30%**、最近 3 轮不动、第 3 档看状态条件不看阈值 | 095 | **opus** | 决策类 |
| [097](097-subagent-ingredient-audit.md) ✅ | **核查**：父 agent 取料取的是子结论还是子 history —— **现状正确**（子的产出只以一条 `tool_result` 进父历史，O(1) 于子的轮数），5 条锁死测试已过变异检验 | — | sonnet | ✅ |
| [098](098-single-turn-overflow.md) ✅ | **决策**：单轮超窗 —— **用户输入限死 1 万字符（拒绝不截断）**；工具返回沿用 32 KiB 单条截断，**不加轮级上限** | — | **opus** | 决策类 |
| [099](099-send-plan.md) ✅ | `SendPlan`（已清列表 + 边界 + 摘要引用）与**投影纯函数**；清工具结果=**换占位保 `ToolUse`**（对 095 的修正） | 095+096 | **opus** | ✅ |
| [100](100-projection-into-ingredients.md) ✅ | 投影接进料单与 `encode`；新增 `Slot::SendPlan`（`Private`，走 `AgentValue::Json` codec，`Slot::ALL` 15→16） | [099](099-send-plan.md) | sonnet | ✅ |
| [101](101-clear-tool-results-command.md) ✅ | **第 2 档**：清工具返回的 command（进 undo log）；`ClearOutcome` 三桶记账，unknown id 不静默吞 | [100](100-projection-into-ingredients.md) | sonnet | ✅ |
| [102](102-clear-tool-results-policy.md) ✅ | 第 2 档触发与选择：85% 触发、保护区之外**一次全清**、最近 3 轮不动；反向锁钉死「不是常开」 | [101](101-clear-tool-results-command.md) | sonnet | ✅ |
| [103](103-prefix-intent-wiring.md) ✅ | `PrefixIntent` 接线：新增 `Slot::PrevSendPlan`（16→17）比较判定；反向锁 + 「压缩轮照样进第 3 层窗口」双向锁 | [102](102-clear-tool-results-policy.md) | sonnet | ✅ |
| [104](104-boundary-command.md) ✅ | **第 4 档**：边界推进 command（清窗口 = 边界推到底）；顺带修掉 100 遗留的 `KNOWN_LABELS` 恢复 bug | [100](100-projection-into-ingredients.md) | sonnet | ✅ |
| [105](105-effect-compact.md) ✅ | **第 3 档**：`Effect::Compact` + 两个 `Event` + epoch 闸；匹配 epoch 发 `Notice` 让「接受」可观测（中途改的接口） | [104](104-boundary-command.md) | **opus** | ✅ |
| [106](106-summary-via-subagent.md) ✅ | 摘要生成走子 agent；**行为验收因 `Effect::Compact` 当时对外不可达而移交 108** | [105](105-effect-compact.md) | sonnet | ✅ |
| [107](107-summary-writeback.md) ✅ | 摘要回写与 epoch 校验：`apply_summary` **三件事一条 entry**；新增 `Slot::Summaries`（17→18），`SummaryId` 由 `upto` 派生；新 label `apply_summary` + 重启回归（100 的坑） | [106](106-summary-via-subagent.md) | **opus** | ✅ |
| [108](108-tier-ladder.md) ✅ | 阶梯编排（跨轮：本轮清、下轮再测）+ `passed_epoch_gate` 显式握手 + 摘要子回收；**独测在真实请求体上抓到「第 3 档哑火」** | 103+107 | **opus** | ✅ |
| [109](109-compaction-visibility.md) ✅ | 被摘要盖住的段在时间线上可见：两个离散 SSE 事件带 `turn_id`（不做快照重播）+ `GET /sessions/{id}/compaction_record` | [107](107-summary-writeback.md) | sonnet | — |
| [110](110-compaction-dogfood.md) ✅ | **真机 dogfood** ← M12 终点；前置补了 `context_window` 接通（五个宿主全是 `None`，压缩在真产品里本来是哑的） | 108+109 | 真机 | ✅ |

**M12 验收 ✅ 兑现**（真机，2026-08-10，[110 真机记录](110-compaction-dogfood.md)）：三家各撑爆一次窗口，
会话不中断，压缩轮**全部判「预期内」零误报**；第 2 档降幅与被清工具结果大小逐 token 对上
（DeepSeek −7756 / Kimi −7241 / GLM −7605），恢复轮命中率 97.9–99.9%；第 3 档由真子 agent
生成摘要（`summary@2/4/6`）；压缩后 `/undo` 正常、杀进程能恢复、**完整记录 1.6 MB 一字未动**。

> ⚠️ **一个跟设计直觉相反的实测**：固定前缀占比高时，压缩轮的缓存命中率仍有 **90%**
> ——[PROVIDERS.md](../../probes/PROVIDERS.md) 那个「一次压缩 ≈120 轮」隐含假设是「历史是大头」。
> 引用那个数字时要一起读。

---

## M13 · 浏览器内核（wasm）

**核心判断：wasm 是第三种宿主形态，不替代任何一种。** 独立跑 / 宿主子进程 / 浏览器内
三者并存，决策 12「`agent-server` 是库」一行不动。

这条**推翻了 ROADMAP 决策 10「砍掉 wasm 目标」**。决策 10 的两条理由都不成立了：
`agent-providers` 里根本没有 HTTP 客户端（要维护两套的是 `agent-transport` 一个已隔离的
crate），而浏览器侧的 transport **比 ureq 那套薄**（`read_loop.rs` 那 165 行是为绕开
ureq 没有中断句柄，`AbortController` 原生就是）。前提也已实测：DeepSeek / Kimi / GLM
三家预检全部回显任意 origin 且放行 `authorization`。

**动手前必读 [111](111-wasm-target-decision.md)**——四条证据、四项代价、以及「哪两件事
因此自动不存在」都在那里，别在实现 issue 里重新讨论。

```
111(决策) ─┬→ 112(ToolExecutor 注入接缝) ✅ ─┐
           └→ 113(fetch transport) ✅ ──────┴→ 115(决策) ✅ → 116(泵 async 化) ✅ → 117(IO 换载体) ✅ → 114(wasm 宿主，M13 终点) ✅
                                              ├ 114a IndexedDB SessionStore ✅
                                              ├ 114b Instant/SystemTime 垫 web-time ✅
                                              ├ 114c wasm 目标 + wasm-bindgen 宿主入口 ✅
                                              └ 114d provider 配置由宿主注入 ✅

**M13 完成**（2026-08-11）。浏览器里真跑通了：Chrome + DeepSeek 流式回复、模型调
`web:page/title` 宿主工具、刷新 4 次后从 IndexedDB 重放 12 条消息且**重开后第一轮的
`tools` 与关闭前最后一轮字符串全等（416 字节，红线 11）**、流到一半取消 65ms 内
`abort()` 生效且下一轮正常。托管只有 `python3 -m http.server` 发三种静态字节，
模型请求直连 `api.deepseek.com`，**没有任何服务端进程**。

留下两条没验成的，见下方「M13 遗留」。

116/117 的真机验收已于 2026-08-11 补齐（真 DeepSeek，非假 server）：前缀缓存
第 2 轮起 0.973/0.978/0.995 全部 ≥ 0.9；SIGINT 取消后进程存活、痕迹擦除、
下一轮缓存仍 0.98——取消轮若在前缀里留残渣，这个数会当场掉到 0。
```

112 与 113 从 111 之后**完全并行**，不碰对方的文件，均已完成。

**115 是 113 实做时撞出来的，111 没预料到**：`post_stream` 的同步签名在 wasm 上
没有线程就没法阻塞等 `fetch`（`thread::spawn` 能编译、运行时 trap，113 实测）。
而 `io_thread` 同时扛着 029 的并行、`sync_channel(0)` 的会合背压和「放弃不 join」
三件事，不是换个 async 就完。**115 不定，114 做不下去。**

| # | 任务 | 依赖 | 模型 | 独测 |
|---|---|---|---|---|
| [111](111-wasm-target-decision.md) | **决策**：恢复 wasm 目标，取代决策 10；浏览器形态的裁剪清单与代价 | — | **opus** | 决策类 |
| [112](112-tool-executor-seam.md) | `ToolExecutor` 开注入接缝，顺带把 ARCHITECTURE 那句「mock 一个 tool executor」变成真的（原写「本里程碑唯一的结构性改动」，**已被 113 证伪**，见 115） | 111 | sonnet | ✅ |
| [113](113-fetch-transport.md) | `agent-transport` 的 fetch 实现，native 那条一行不动 | 111 | sonnet | ✅ |
| [115](115-wasm-io-without-threads.md) | **决策**：wasm 上没有线程，provider IO 路径怎么办 —— 泵怎么等 / `sync_channel(0)` 换成什么 / 029 并行怎么保 / 决策 16 的理由是否还成立 | 113 | **opus** | 决策类 |
| [116](116-async-pump.md) | 引 `futures` 最小子集、泵与 `run_turn` async 化 —— **纯 native，不碰 wasm** | 115 | sonnet | ✅ |
| [117](117-io-without-threads.md) | `io_thread` 换并发 future、channel 换 futures mpsc —— 029 并行保全 + 幽灵增量对抗测试 | 116 | **opus** | ✅ 已完成 |
| [114](114-wasm-host.md) | wasm 宿主打通 + IndexedDB 持久化 ← **M13 终点，已完成** | 117 | a=sonnet b=sonnet c=opus d=sonnet | ✅ |
| [118](118-living-docs-io-carrier-drift.md) | 活文档里「IO 线程池」的措辞已对不上形状（结论没错，形状错了） | 114 | sonnet | 进行中 |
| [119](119-mcp-as-a-feature.md) | `agent-mcp` 做成 feature —— 把「不可达」从每次都要论证的性质变成编译期事实 | 114 | sonnet | — |

**M13 验收**（可判定）：浏览器里**没有任何服务端进程**跑完一轮真实对话；模型调用一个
只有前端拿得到的 `web:` 工具并用结果回答；刷新后同会话 id 从 IndexedDB journal 回放，
**第一轮工具表与关闭前逐字节相同**；取消能真的中断请求；`srv:` 的 shell/fs **不出现在
工具表里**。

 ---
 
 ## 怎么做

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

**M11（图片附件）的四条 079–082 动手前必读 [../IMAGES.md](../IMAGES.md)**——决定、
实测证据、以及「上传该放在哪」这个最容易放错的落点都在那里，别在 issue 里重新讨论。


## M13 遗留（两条，都不阻塞，但别忘了）

**一、只跑通了 DeepSeek 一家。** 三家各跑一轮那条验收没满足——本机
`providers.toml` 里 kimi/glm 两段 `api_key` 是空的。用占位 key 各发过一轮：两家的请求
都**穿过 CORS 拿到真实 401** 并被 adapter 正确分类成 `Failed(Provider(Auth))`，
说明 transport 与 adapter 在 wasm 下无差异，**差的只是一把能用的 key**，不是代码问题。
拿到 key 直接补跑，不需要改任何东西。

**二、`agent-mcp` 仍被编进浏览器产物，是死重量。** 决策 26 说浏览器构建不编它，
但真要摘掉得在 `ctx.rs` / `dispatch.rs` / `runner.rs` / `io_task.rs` / `mcp_call.rs` /
`lib.rs` **六处撒 `#[cfg(target_arch)]`**——正是红线 12 与 114 硬约束要避免的形状。
当前状态是「代码在、路径不可达」：工具表里没有任何 `mcp:` 名字、`McpRegistry` 是空表、
`dispatch` 第四路要求 `starts_with("mcp:") && table_declared` 两个条件都不可能成立，
所以 `mcp_call::start` 里那句在 wasm 上会 trap 的 `thread::spawn` 永远走不到。
产物里 `strings` 得到 `tools/call` / `jsonrpc` 就是这些死代码。

**正确的解法是把 MCP 做成 `agent-runtime` 的 feature，而不是撒 cfg。** 另开 issue 处理，
不要为了「产物干净点」在核心执行路径上撒平台判断——那个代价比 772K 里的几 K 死代码大得多。

> 顺带核实：产物里还能 `strings` 到 `srv:agent/spawn` / `srv:agent/collect` /
> `srv:agent/status` / `srv:vision/inspect`。**这些不是漏网的 shell/fs**——是子 agent
> 编排（029 的并行）与视觉检查，本来就该在。被裁掉的 `shell/exec` 与 `srv:fs/read`
> 确实 `strings` 0 命中，spec 构造器从没被调用，整个被 DCE 删了。
