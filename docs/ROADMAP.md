# 路线与决策记录

两件事：**已经拍板的决策**（避免重新讨论）和**没做的部分按什么顺序做**。

架构细节在 [ARCHITECTURE.md](ARCHITECTURE.md)，状态机制在 [STATE-MODEL.md](STATE-MODEL.md)，
硬约束在 [INVARIANTS.md](INVARIANTS.md)。这份只管「定了什么」和「接下来做什么」。

## 一、已拍板的决策

重新讨论这些之前，先看理由——多数是权衡后的取舍，不是默认值。

| # | 决策 | 理由 |
|---|---|---|
| 1 | **状态引擎从 `einfach-core` fork，独立演进** | 不回合上游、不同步其 bug 修复。泛型化上游要兼容存量表格栈，成本不划算 |
| 2 | **单 monorepo，Cargo + pnpm 双 workspace** | 唯一理由是协议类型从 Rust 生成 TS，变更要能在一个提交里原子完成 |
| 3 | **整棵 agent 树共用一个 store**，family 按 `AgentId` 区分 | 子读父是一次 `get`；等待子 agent 是一个 derived atom；跨 agent undo 天生一致 |
| 4 | **undo 是单一线性日志 + 单游标，全局回滚** | 选择性 undo（跳过日志中间条目）不成立，中间条目的 `prev` 是当时世界状态下捕获的 |
| 5 | **undo 两层粒度**：turn 默认，batch 可展开 | 共用一条日志，靠 `turn_id` 分组。`turn_id` 由 root 分配，子 agent 继承 |
| 6 | **持久化与 undo 是同一份代码** | 恢复 = 从快照把 `next` 往前推，那就是 redo 的循环 |
| 7 | **工具按执行位置三分**：Server / Web / Desktop | `reversibility`（Pure/Reversible/Irreversible）是**正交**维度，不合并；不叫 `Effect`，那个词留给 loop |
| 8 | **agent 之间只允许上下读，禁止横读** | 依赖图恒为树，两个方向可读的 slot 集合不相交，环在结构上不可能 |
| 9 | **传输：SSE 下行 + 普通 POST 上行** | 不要 WebSocket。服务端「反向调用客户端」只是在流上推事件 |
| 10 | ~~**砍掉 wasm 目标，Tauri 内嵌 server**~~ **被 26 取代** | 见 26。「provider 不用维护两套」与代码不符——`agent-providers` 里没有 HTTP 客户端 |
| 11 | **server 不做鉴权 / 日志规范 / 集群** | 企业边缘层，每家规范不同。只读 identity header 不验证，只遵守 W3C `traceparent` |
| 12 | **`agent-server` 是库不是二进制** | 桌面版内嵌它，企业内部服务也内嵌它。只给二进制的话他们只能在外面套代理 |
| 13 | **Java 网关只是参考实现**，不发 Maven、不跟版 | 避免 Spring Boot 2/3 双分支与 JDK 矩阵的长期维护税 |
| 14 | ~~`Capabilities` 是 core 读的唯一接缝~~ **被 17 取代** | 见 17。能力位分支只是 `match provider` 换了层皮 |
| 15 | **请求组装归 adapter，core 只供料** | 组装的每个决策都依赖能力位（工具晚加放哪、skill 注入到哪、thinking 进不进前缀、temperature 能不能改），core 里做只能做成不看能力位的搬运 |
| 16 | **`Encoded`（原 `ProviderRequest`）存在的理由是线程边界，不是组装**（2026-08-11 补注：**是双理由**，见右） | store 是 `Rc<RefCell>` 不 `Send`，HTTP 在别的线程。必须有一份「在 actor 线程上提取、能带走」的东西。**补注**：决策 26 的 wasm 形态下没有那条线程边界，第一个理由不成立；但它**同时是 `check_drift` 的快照**，这第二个理由与目标无关。**结论保留，别因为第一个理由不成立就删掉它**（issue 115 §要定的四件事 · 4） |
| 17 | **core 里不许有任何模型相关的判断**（红线 12）：从「事前问能力」改成「事后报调整」 | core 只说意图，adapter 做不到就报一条 `Adjustment`（encode 时产生，宿主随 `ProviderDone` 事件喂进 loop）。事前分支 N 位就是 2^N 种组合、多数没跑过、加一家要改 core；事后报调整是可见可审计的，加 provider 不动 core，测试组合掉回 1 |
| 18 | **压缩三分**：触发在 core（当前 tokens vs `SessionConfig` 的窗口大小，纯算术——红线 12 禁分支不禁参数）；实现在 core（统一一份，压缩是状态变更，走 command 层进 undo log）；压后摆盘在 adapter（前缀树的家能保共享分支，仅扩展的认赔并报 `Adjustment`） | adapter 是纯函数无权改世界——它偷偷压，prompt 和状态对不上，undo / 审计 / 前缀镜像一起断 |
| 21 | ~~**skill 激活 = 模型经工具 + 常驻索引，宿主可显式预激活，否决自动触发**~~ **被 27 取代（原则全保留，通道换形）** | 见 27。保留的：索引解鸡生蛋、AI 决定用哪个能力、否决自动触发。换掉的：正文通道从「激活状态 + 拼进 system 段尾部的注入」改成「普通工具结果」，激活状态整个不再存在——038 实测出的三家注入分策（DeepSeek 段尾 91%）随之整个不再需要 |
| 20 | **子 agent 由模型经内置工具 spawn**（006 拍板）：`spawn_agent` 是 Server 工具，spawn 即 tool call 进日志，「等子树完成」= 该槽位收敛，结果以 tool_result 回父 | ①undo/审计免费——走既有 ToolCall 机制，turn_id 继承让「撤一轮连带子树」天然成立；B 路要为编排动作另发明记账路（第二真值来源）②与开山原则一致：AI 决定调用哪个工具，分解只是又一个工具 ③A 不封死 B（编排层=另一个会调 spawn 的调用方），反向不成立。成本兜底：深度≤3/子数≤8/子树轮预算全是参数，超限 = is_error 的 tool_result 让模型自己收敛 |
| 19 | **工具结果上限：默认 32 KiB、只留头部、core 边界截断、标记确定可见** | ≈8k 英文 token，一次调用最多吃 128k 窗口的 ~8%；`fs/read` 有行范围可分次拿。executor 不知道 prompt 预算所以在 core 截；标记进 prompt 必须逐字节确定（红线 11），写明原始大小与「缩小范围重调」指引。头尾各半到 020（shell）再议 |
| 22 | **MCP 当 adapter 接**：新 crate `agent-mcp`（做 IO，不在红线 7 内）；可逆性从 `readOnlyHint` 翻译成 per-tool 元数据（**不从名字推**——`ToolTable` 携带映射）；`tools/call` 走**异步在飞路**（`provider_call` 同款，不同步阻塞 actor）；活句柄住 store 外的 `McpRegistry`（红线 3）；MVP = **stdio + tools**，`.mcp.json` 跟 Claude Code 对齐，http/resources/prompts 延后 | MCP 是外部来源差异合法存在的地方，和 provider 同类接缝，只是要做 IO。可逆性是 per-tool（同前缀不同 `readOnlyHint`），机械按名字判会把数据事故开关交给第三方——默认落 `Irreversible`。异步执行因 MCP 慢无上限、且红线 6 的 epoch 回写天然在异步路上。接缝完整定义见 [MCP.md](MCP.md) |
| 23 | **子 agent 可观测 = 派生读，不新增状态**：`Session::agent_tree()` 是对现有 atom 的一次纯派生读（往下读，红线 10 方向）；**不为「当前动作」加 primitive**（那是第二真值源，undo 破）；树由 core 权威算、UI 哑渲染（不让 UI 从事件流重建状态机）；M7 范围 = **活树**（当前快照 + 变化推 SSE），可回放时间线（任意 epoch 快照）延后 | 子 agent 的状态早就是 atom（整棵树共用一个 store），"看它在干啥"是把已有状态摆出来，不是造监控。派生读 → undo/恢复/回放一致性白拿（第五个投影）。UI 重建状态机脆且 reconnect 断，快照做真值最省。接缝完整定义见 [OBSERVABILITY.md](OBSERVABILITY.md) |
| 25 | **企业集成三条**（M9）：①**拉取式是 ring 的第二个投影**——`GET /events/poll` 复用 `RingState::replay` 与 **`Last-Event-ID` 同一个游标 header**（仓库 axum 没开 `query` feature，且「没有查询参数协议」是既有约定），SSE 端点保留不动；②**会话身份 = 业务侧 chatid**，`POST /sessions` 幂等三态（活着接上 / 磁盘有则恢复 / 都没有才建），id 走白名单**拒绝不 sanitize**；③**生命周期归 Java**——`ProcessBuilder` 起子进程（`--port 0` + `--ready-file` 原子握手 + SIGTERM 优雅落盘）；Rust 提供最小启动协议而不进入 JVM | SSE 的复杂度只该出现在「**产生** SSE」那一跳（Spring 标准做法），不该出现在「**代理** SSE」那一跳（四个坑全在这里 + 强制 WebFlux，而企业存量多是 MVC）。拉取式的断开检测**整套复用 `SubscriberGuard`**（每次 poll 期间持有 → 计数/宽限/取消路一行新逻辑都不用写），比自造 last-poll 时间戳少一个真值源。JNI/FFI 真嵌入**否决**：流式跨 FFI 难做好、Rust panic 会杀 JVM、进程隔离全丢。接缝完整定义见 [INTEGRATION.md](INTEGRATION.md) |
| 24 | **模型侧异步编排 = turn 内**（M8）：给模型三个工具（`spawn` 加 `background`、`status` 非阻塞下读子树、`collect` 领后台子结果），让它**中途观测子 agent 并改变编排**（不是加并行——并行 spawn 早有）；但**子 agent 仍不跨 turn**——后台子在父这一次 `run_turn` 内必须 collect 完或被孤儿取消。前台 spawn（决策 20）≡ `spawn(bg)+collect` 融进一槽，一行不改 | 决策 20 的干净全靠「子在父同一 turn 内生死」（`turn_id` 继承、undo 连带子树、`Subtree` 局部绑定、pump 静止条件 `calls.is_empty()` 把「root 终态+子树跑」列为无定义）。跨 turn 后台子要 store 落地的跨-`run_turn` 映射 + 重写 undo 语义 + per-child 取消——收益未证，延后。turn 内已给全「观测+反应」能力且红线全不破。接缝完整定义见 [ORCHESTRATION.md](ORCHESTRATION.md) |

| 26 | **恢复 wasm 目标**（2026-08-10，取代决策 10）：核心编进浏览器直接跑，**wasm 是第三种宿主形态**——独立跑 / 宿主子进程 / 浏览器内三者并存，决策 12「`agent-server` 是库」不动。浏览器形态下不编 `agent-mcp`（stdio 不存在；浏览器够得着的 MCP 由前端自己连，HOST-CAPABILITIES §七 早定的方向），不声明 `agent-tools` 的 `srv:` shell/fs specs（纯数据，不声明即可），`agent-transport` 换 fetch 实现 | 决策 10 的两条理由都不成立了。①**「provider 不用维护两套」与代码不符**：`agent-providers` 依赖只有 `agent-core`+serde，**没有任何 HTTP 客户端**，IO 全在 `agent-transport` 一个已隔离的 crate（红线镜像约束「唯一允许依赖 ureq」）；②**浏览器侧 transport 更薄不更厚**：`read_loop.rs` 那 165 行读线程 + `mpsc::sync_channel` + 双超时旋钮，存在的唯一理由是 ureq 阻塞 read 没有外部中断句柄（自称「不优雅但可测」），`fetch` + `AbortController` 原生就是那个句柄；③**前提已实测**：DeepSeek / Kimi / GLM 三家预检全部回显任意 origin 且放行 `authorization`，浏览器直连可行——这是决策 10 当时没验的前提；④决策 16 的 `Rc<RefCell>` 不 `Send` 在原生是让步，单线程 wasm 里变成 fit。**代价照实记**：`RunnerCtx.fs: ToolExecutor` 是 concrete struct（`new()` 要 canonicalize 真实目录），必须开注入接缝——本次移植唯一的结构性改动；`Instant`/`SystemTime` 要垫 `web-time`；多一个编译目标要长期维护；key 落在浏览器，定为每人一把自己的。逐条证据与验收见 [issues/111](issues/111-wasm-target-decision.md) |
| 27 | **工具加「调用时机」维度；skills 降级为外层工具包**（2026-08-11，取代 21）：`ToolTable` 判定侧加 `CallTiming`——空 = 模型自主调；`SessionStart` = 会话创建时按注册顺序自动调、结果落状态、组料时成为 system 前缀块（恢复读状态不重跑；失败 = 创建报错；执行体是注册时给的本地函数，远端时机工具结构上不存在）；`TurnEnd` = 每个完成轮后自动调、结果丢弃（v1 不回灌、不续 loop）。**时机非空不进模型清单**。skills 从此不是 core/runtime 概念：索引 = 一个开局工具、正文 = 普通 `srv:skill/read`（Pure）按需读、以 tool result 进对话；树形靠正文引用 + frontmatter `hidden`；用户点名走 user 消息；激活子系统（`skills_active`/activate/deactivate/拼进 system 段尾部的注入）已删除（141 落地，2026-08-12） | ①业内已收敛（Agent Skills 开放标准，40+ 平台）：目录进 system + 正文按需，主流通道就是「读取 → tool result」——消息尾追加是缓存专门设计的方向；**真机已验**：DeepSeek 十轮 cached/prompt 全部 ≥ 97.8%（含 read 轮），段尾税与每轮 drift 上报消失 ②概念收敛：core/runtime 只剩「一张表 + 三个时机」，「谁发起」是继 location、reversibility 之后第三个正交维度，且不进 `ToolSpec`——模型面契约一字不动 ③正文撤回并进 undo（read 是普通 entry），独立停用机制不再需要 ④M10 的 skill 专用通道收编为「宿主给工具包喂数据源」（skill 携带工具 v1 裁剪，声明即 400）。逐条任务见 [issues/README §M15](issues/README.md) |
| 28 | **子 agent 的开局材料按名单授予**（2026-08-12）：`srv:agent/spawn` 加可选入参 `inherit_prefix`（字符串数组）——**缺省 = 全带**（今天的行为）；`[]` = 全不带；`["srv:skill/index",…]` = 挑着带。值随 spawn 快照落 `Slot::PrefixAllowed`（编码照 `ToolsAllowed`：排序去重、`Null` = 不设限）；`system_for` 按纯名字匹配过滤 `init:<name>` 前缀块；名字不在 timed 区 → `is_error` 让模型自纠（决策 20 兜底同款）；字段只管 `SessionStart` 产物——`TurnEnd` 无 per-child 语义（子不跨 turn，没有轮末）。落地 144/145 | ①子任务常不需要会话开局材料（如技能索引）——正文的懒加载 M15 已解决，索引行本身也该能按子省掉 ②**缺省必须全带**：被否的「混进 `tools` 名单」案（名单即继承，零新状态是它唯一的优点）有个真脚枪——收窄工具的子会静默丢索引，模型必须记得补名字否则静默降级，正是本仓最怕的形状 ③独立字段保 `tools` 类型纯净（名单里不出现调不了的名字），两字段各对各区校验 ④「给 read 不带 index」是真组合：父在任务文本里点名 skill id，子不需要目录 ⑤决策 20 一致：模型 spawn 时自选——静态部署字段与「从工具子集推导」两案都答不了「每次 spawn 动态决定」这个真问题 |
| 29 | **扩展双层：Rust 内核零脚本运行时，TS 生态在宿主层**（2026-08-12，对标 pi 的扩展路线拍板）：内核不嵌任何脚本引擎（**否决** Javy/AssemblyScript/进程内 TS）；Rust 扩展的能力面 = executor 形状的纯 IO 工具 + **截获式工具**（唯一的状态正门：拿 `Session` 手套——读走类型化读口且照 status 先例按调用者后代收窄，写只走 command 面、自动 journaled）+ timed 工具 + MCP；交付物 = `ExtensionPack`（spec 与执行路径成对、`ext:<pack>/` 强制前缀、可逆性显式声明缺省 Irreversible）；TS/浏览器生态整个长在宿主层（web-agent 经 M10 capabilities + M14 页面工具回调），runtime 不认识「扩展」一词（同 skills 先例）；**wasm 纯函数沙箱记档不建**——那是将来接不受信第三方 derived 公式时的备选（无 import 即无时钟/随机/IO，纯度是物理不是纪律），受信 Rust 扩展用不上。落地 M16（146–152） | ①信任模型分层：自家 Rust 进主仓 = 与自有代码同一套纪律（check-invariants 扫描面扩到扩展模块即可），不受信代码今天没有用户，为它先建沙箱是在猜 ②TS 嵌入两条路都是给不存在的用户交税：AssemblyScript 不是完整 TS（npm 生态用不了）、Javy 每个扩展背一个 ~MB 级解释器 ③web-agent 的地基（`@einfach/core` TS 版）derived 原生、React UI 零胶水——pi 式玩家生态在 TS 侧长本来就更顺，两层楼各干各的（楼下账本楼上生态，中间 capabilities 一条缝已真机验过）④账本完整性要求唯一写门（红线 2）：进程内脚本绕门的风险从「审计防不住」变成「结构上不存在」⑤引擎带的 `createUndoRedo`（WeakMap 按对象身份存历史）经查证**结构上不可持久化**，是「引擎 1:1 但账本自建」的实证——扩展访问状态必须过账本的门，理由同源 |
| 30 | **扩展观测「被问才算」，不做反应式层**（2026-08-12，150 拍板，比原计划小一圈）：①hook 没有独立概念——就是 `TurnEnd` 时机的工具；②`TimedRun` 签名加**只读** `&Session`（轮末驱动手里本来就有，递进去而已；SessionStart 同签名统一，创建期读到空会话无害）；v1 边界不动：hook 不写状态（自此由签名保证而非纪律）、不回灌、不续 loop；③扩展**没有** derived 注册面——截获工具拿 Session 被调时现算，工具体内自便（core 自己的 derived 引擎照旧，只是不对扩展开注册口）；④状态谓词触发（「没人喊自己动」的反应式层）**不做**，连同「扩展 derived 公式」的完整讨论记档防重开；151/152 随之撤销，落地只剩 153 一刀 | ①149 的真实需求单上全部是「被问才算」：report 是模型喊的、审计是轮末驱动喊的——反应式层（注册面 + 纯度扫描扩面 + 去抖语义 + 防偷改 prompt 四件套）服务的是一个不存在的用户，正是 021「没被真实使用验证过的东西跟没写一样」②「被问才算」的工具体内已经能算任何东西（Session 手套读全开、轮间是稳定态），电子屏与服务员的差别只剩「没人喊自己动」，而那恰是决策 21 否决自动触发时警惕的形状 ③`TimedRun` 缺 `&Session` 是 149 实测出的唯一真墙（ext:stats 被迫内存传话 + `seen_at` 认账），一刀补齐后传话格整个删除 |
| 31 | **宿主声明开局块：声明的是内容，不是执行体**（2026-08-12，M17，清掉 §四「宿主声明不了 timed 工具」）：`capabilities.prefix: [{name, text}]`——宿主建会话**之前**自己跑完逻辑，把结果文本带进来；装配期合成「执行体 = 返回常量文本」的 `SessionStart` timed 工具（表尾、按名排序），于是 `run_session_start`、恢复回放（134 状态 + `Slot::HostPrefix` 照 073 落店）、`inherit_prefix` 校验（读 timed 区 spec 名）、`session_has_history` 闸**全部零改动认识它**；撞名判据 = 名字强制 `web:`/`desk:` 前缀（与内置 `srv:` 结构性不撞）+ 声明内/与 `tools` 重名拒 + 空 text 400；**远程执行否决**——135 已判 SSE 宿主创建时回传通道结构上不存在，拉取式虽可表达但要把建会话改成异步状态机（挂起/超时/半建清理），服务的只是「拿一段文本」；**TurnEnd 不进协议**——宿主在墙外 poll/SSE 天然看得见每轮结束，副作用在自己家做，这就是宿主版轮末 hook（决策 30「被问才算」的宿主投影）。落地 M17（154–158） | ①`SessionStart` 工具的全部意义是它的**结果**——一段进前缀的文本，在哪跑不重要，让宿主在自己家跑完再声明就把「代码过不了网线」这个死结整个绕开 ②声明是会话状态（073 的既有拍板）：journaled → 恢复回放白拿、历史不可改写白拿 ③合成 timed 条目 = 字面上兑现「宿主声明 timed 工具」，且三处校验/过滤自动认识它，零新概念零新面 |
| 32 | **子 agent 上限的配置面开在进程级**（2026-08-12，M18，159 拍板，清掉 §四「子 agent 上限的配置面」）：决策 20 的 `AgentLimits { max_depth, max_children }` 一直是代码可配、**运行时无入口**（四个生产装配点写死 `default()`）。定为**启动参数**——`agent-server`/`agent-cli` 各加 `--max-agent-depth`/`--max-children`（env 兜底，命令行优先），协议面**零改动**；**per-session 进协议否决**（它要求 per-session 的值跨恢复活下来，而 limits 进不了 store；且决策 20 的定位是部署方的成本兜底，让建会话的客户端自己填等于兜底由被兜的那一方决定；多租户今天无用户）；**进 store 当 `Slot` 否决**（073/154 那条路是为「要进 prompt、要能回放、要能 undo 的内容」设计的，limits 三条都不占，`spawn.rs:91` 早已拍过上限变更不可撤——会长出一个不进 undo 的半吊子 slot）。**校验取严**：解析失败/`0` 拒绝启动而非静默退默认——本仓两种取向都有先例，判据是**有没有下游替它报错**：`--port` 能静默退 `None` 是因为 `default_bind_addr` 会替它报，`AGENT_BIND` 配错则硬失败（`BindConfigError` 文档：「把打错的字符串当成没设，是那种配置错了却看起来在正常运行的坑」）；上限没有下游，故跟 `AGENT_BIND` 走。下限钉 1，要关 spawn 走 076 的 `disable_builtin`；上限不设死限。**附带修一处静默失配**（160）：`restore` 曾把 `limits` 硬写 `default()` 且 `recover` 无入参，「宿主载入后重调」这条路名存实亡——配置值恒等于默认值时不显形，上限一可配第一次重启就显形。落地 M18（159–163） | ①与「limits 是配置不是状态」的既有拍板一致（`spawn.rs` 字段文档 + `restore.rs`），不推翻任何东西 ②进程级配置天然跨恢复一致：同一份启动参数重启，恢复出来的会话拿到同一组数——前提是 `recover` 真有那个入参，这正是 160 补的 ③两侧数字（工具描述那份 + `spawn_child` 拦人那份）的对齐 034 已做成一次函数调用，配置面只要喂进 `ToolTableSpec::Full` 就全链自动通，不新增耦合点 |

## 二、现状

### 仓库里现在有什么

```
crates/                   六个 crate（M1 产物，见下）
probes/                   两个探针 + PROVIDERS.md（三家差异的唯一结论文档）+ 原始观测
docs/                     决策、状态模型、工具模型、适配层接缝、红线 12 条、issues
scripts/                  check-invariants.sh（PostToolUse hook + 本地收工检查）
providers.example.toml    key 模板（providers.toml 已 gitignore）
```

历史注脚：M1 开工前曾整仓清空过一次——那三个抢跑写的 crate 没有 issue、没有独测、
验收事后补，整体删除按流程重写（教训在 [WORKFLOW.md](WORKFLOW.md) §四）。重写后的
版本经独立测试 agent 与真实调用双重验收，质量差异见各 issue 的实做记录。

### 已完成：M16 Rust 扩展包（2026-08-12，前半真机 dogfood 六条全过，153 收口）

决策 29 落地：截获注册表（[146](issues/146-intercept-registry.md)）、既有四条截获
迁进同一张表（[147](issues/147-migrate-intercepts.md)）、`ExtensionPack` 接缝
（[148](issues/148-extension-pack-seam.md)），并由第一个**真扩展包**走完全程
（[149](issues/149-extension-dogfood.md)）。接缝定义见 [EXTENSIONS.md](EXTENSIONS.md)，
§五「写你的第一个扩展包」就是这次 dogfood 的教材化。

**真机数字**（149，CLI + DeepSeek，`agent-cli` 里的 `ext:stats` 包，`--ext-stats` 开关默认关）：

- **undo 的活演示**（账本卖点的正面戏）：模型 spawn 一个子 agent 之后调扩展工具拿到
  「4 轮、19 条 entry、2 个 agent、工具调用 2 次」；两次 `/undo` 之后同一个工具报
  「3 轮、11 条 entry、1 个 agent、工具调用 1 次」，`spawn_child×1` 整条从 entry 分布里
  消失。**扩展这一侧没有一行代码认识「撤销」**——第三方读到的世界与账本严格一致是白拿的。
- **不装包 = 逐字节零变化**：拿本地 recorder 收请求体，改动前的二进制与改动后不开开关的
  二进制，body 的 sha256 **相等**（23957 字节）；开了开关那份前 23955 字节逐字相同，多出
  的一条工具整段在表尾（红线 11）。
- 十轮 14 跳缓存命中 **96.1%–99.0%，均值 97.9%**（含五个扩展工具调用轮）——扩展工具的
  结果跟 skill 正文一样从消息尾进来，不破前缀。
- `TurnEnd` 钩子每个完成轮恰一行审计、`Ctrl-C` 取消的那一轮 0 行；`kill -9` 恢复后再调，
  崩溃前的数字原样在。

**收口**（[150](issues/150-derived-extension-decision.md) 拍板决策 30、
[153](issues/153-timed-run-session.md) 落地，151/152 随决策 30 撤销）：149 交上来的手感是
「`TurnEnd` 钩子的 `TimedRun` 签名里没有 `Session`」，153 是这句话唯一的实现刀——签名加
**只读** `&Session`（`&ToolTable` 与 `&Value` 中间），两个驱动
（`run_session_start`/`turn_end::fire`）顺手递参，`ext:stats/audit` 从此在轮末**现读**
一次账本，149 那格 `Ledger` 传话与 `seen_at=` 标注整个删除，审计行改为
`turn=N entries=X/Y agents=Z tools=W`。全仓 timed 执行体（生产代码 + 独立测试 fakes）
机械跟随新签名，`cargo test --workspace`／`check-invariants.sh --all`／`build-wasm.sh`
三门禁全绿——**M16 到此收官**。

### 已完成：M15 调用时机与 skills 工具化（2026-08-12，真机 dogfood 七条全过）

决策 27 落地。core/runtime 收敛为「一张工具表 + 三个调用时机」，skills 从此不是
core/runtime 的概念：索引是一个开局工具、正文按需 `srv:skill/read`、以 tool result
进对话；树形靠正文引用 + frontmatter `hidden`。老的激活子系统（`skills_active`/
activate/deactivate/`late_system` 注入）整条删掉（[141](issues/141-remove-activation-subsystem.md)），
`Slot::SkillsActive` 只留壳给老快照反序列化用。

**真机数字**（[143](issues/143-m15-dogfood.md)，CLI + server + DeepSeek）：
十轮 13 跳缓存命中 **97.5%–99.8%，均值 98.5%，含 3 个 read 跳，零条低于 0.9**
——决策 27 那个「正文走消息尾不破前缀」的赌注兑现。模型自主走完两跳 read
（索引里根本没有那个 hidden 子 skill，它是顺着 router 正文的引用找过去的）；
undo 撤回正文后同一个模型说「我的上下文里不存在任何口令信息」；`kill -9` 恢复后
前缀块逐字节原样、开局工具计数仍是 1。

### 已完成：M14 浏览器的宿主能力（2026-08-12，十四条每条真机验收）

浏览器宿主补齐两件：**页面可以声明并执行自己的工具**（`onToolCall` 通用回调，
真机实测回调里 `await` 了 504 毫秒——同步执行不可能报出 ≥500），**以及图片**
（选图存 IndexedDB → 模型自己调 `web:source/vision` → 字节发 Kimi 3 → 拿回文字）。
核心判断：这两个需求是同一条缝的两个投影，后者是前者的前提
（[119](issues/119-browser-host-capability-decision.md)）。主模型从头到尾没见过图，
图片字节不进 journal——跟 CLI/server 是同一套设计，不是浏览器的降级。

### 已完成：M7 子 agent 可观测（2026-08-03，插在 M6 中间；真机验收全过）

「子 agent 不该是黑盒，界面要显示它在干啥」——真实使用反馈驱动的插入项。定性：可观测性
是对现有 atom 的一次**派生读**，不是新机制（决策 23、[OBSERVABILITY.md](OBSERVABILITY.md)）。
插在 M6 中间做，是因为 043 起 MCP 调用异步在飞，有了活树面板，M6 真机验收时能直接看到
MCP 调用挂在哪个 agent、在飞多久。四个 issue：046 core 派生读 / 047 CLI `/agents` / 048
SSE 快照事件 / 049 web 树面板。范围 = 活树（当前快照 + 变化推送），可回放时间线延后。

**真机验收**（主会话 playwright + curl 直打，deepseek 真实上游）：模型 `srv:agent/spawn`
起两子 agent → 活树面板 / `GET /agents` **实时**从 1 节点长到 3（`['root','root/a1',
'root/a2']`）、状态灯随 `Thinking→Working→Done` 变（点 1、2）；`POST /undo` → 树**回退到
1 节点**（`['root']`，点 3）。dogfood 逮到一个漏投影：undo/redo 走 actor 命令处理、不经
pump，原本不发树快照——[048](issues/048-tree-sse.md) 补 `RunnerCtx::emit_tree_snapshot`
＋三处调用修掉，`cargo test -p agent-runtime -p agent-server` 全绿后真机复验通过。「测试绿、
世界不对」的又一例：046 单测 + 048 emit 测试都绿，只有真机现形。另捞一条 adapter/spawn 的
工具名编码摩擦，单列 [050](issues/050-tool-name-encoding.md)（模型自纠有效、非阻塞）
——**已拍板并落地**（2026-08-04）：转义规则三家共用一份（不是厂商差异），归一化放在
宿主侧（`agent-runtime::tool_name`），core 与命名约定都不动。理由与被否的三个方向见
该 issue §拍板。
真机彩蛋：模型第一次 spawn 传错工具名被 `is_error` 拒后**自我纠正重试**（决策 20 重演）。

### 已完成：M6 MCP 接入（2026-08-03，真机 dogfood 收官）

原始蓝图「前后端都可以 tool+skills+mcp」的最后一块。定性早在第一天钉好（TOOLS.md §MCP
「当 adapter 不是核心抽象」、红线 3 点名「MCP 子进程句柄」、`source:Mcp` 地基）。接缝定义
见 [MCP.md](MCP.md)、决策 22。MVP = stdio + tools，照 022「先打通一家」的先例，最小「能用」
终点：`.mcp.json` 配真 server → 模型自己发现并调 MCP 工具 → `/undo` 尊重可逆性。
六个 issue：040 决策 / 041 协议+翻译 / 042 stdio 握手 / 043 执行路由+epoch / 044 配置装载 /
045 CLI 终点。http/resources/prompts/OAuth 延后（等真实反馈）。

**已完成**：040（决策，见决策 22）、041（协议+翻译层，62 测试全绿）、042（stdio 传输+握手，
`StdioTransport`+`McpClient`+`McpRegistry`，真 npx `server-everything` 拉 14 工具译成
`mcp:everything/<t>`，握手**记录不断言**协商版本——实测该包已改成「回显客户端提的版本」，正是
不断言的理由；红线 3 结构性证明 client 句柄住 store 外、agent-core/store 不依赖 agent-mcp）。
**已完成**：043（执行路由+可逆性映射+epoch 回写，opus，碰红线 6）：dispatch 第四路
`Dispatched::McpCall`（`mcp:` 前缀 + `declares` 截获，`mcp_call` 模块仿 `provider_call`，
credential 键 `(agent,call_id)`）；`ToolTable` 携 `mcp:` 名→可逆性映射，未命中落保守
`Irreversible`、location 恒 `Server`；红线 6 回写点在 `Session::step` 的 epoch 闸（在飞子 bump
epoch → 幽灵结果被丢）——`tests/mcp_epoch_writeback.rs` 对抗断言（结果确回来了 + 消息历史无它）。
impl agent 收尾自旋（clippy 那道门跳过没确认、恰是红的：`too_many_arguments`），主会话代收：
掐自旋、修 clippy（house-style `#[allow]`）、前台重跑三门禁全绿。
044（`.mcp.json` 装载+多 server+失败隔离，sonnet，**无自旋**——单 crate 快门禁前台跑完）：
`config.rs`(275) 纯解析（streaming `visit_map` 逮撞名，不走 `serde_json::Value` 的 dedup-to-last）/
`loader.rs`(161) 多 server 装载 + 失败隔离（`Availability = Connected|Unavailable|Unsupported`，
一个 server 挂只标自己那行、会话照起）/ `availability.rs`(66) host×transport 门 / `status.rs`(69)
可序列化状态；`env`/`headers` 用 `BTreeMap`（红线 11）。主会话从磁盘复验（行数/无 HashMap/
`cargo test -p agent-mcp` 57+ 绿/clippy 净）。
045（CLI bootstrap 接线 + `/mcp` 状态 + kill-9 重连，sonnet）：`mcp.rs`(206) 读 `.mcp.json`
（默认启动目录，`--mcp-config` 覆盖）→ 跑 044 loader → 工具经既有 `with_mcp` 追加进表尾
（红线 11：跨 server 按 id 排、server 内按 `tools/list` 序）、registry 进 `RunnerCtx`；
`print/mcp.rs`(96) 纯 `/mcp` 渲染。impl agent **无自旋**（第一次 `cargo test` 过 120s 被自动
后台化，它诚实地前台重跑 + 抓真输出）。**主会话真机 dogfood 收官**（deepseek + npx
`server-everything` 13 工具）：`/mcp` 列 connected；模型自发 `mcp:everything/echo`
（`reversibility=Pure`，从 `readOnlyHint` 翻译不按名字猜）→ 拿真结果 `Echo: ...` 组织回答；
第 2 轮缓存 `预测=实际 7040`（红线 11 稳定前缀）；`/undo` 干净越过 Pure 调用；kill-9 全新进程
`会话已恢复` + MCP 从 `.mcp.json` 重 spawn 重连；无孤儿 npx。**M6 由真实运行验收，收官。**
延后（等真实反馈）：http/sse 远端传输（浏览器 host）、resources/prompts、OAuth。

### 已完成：M9 企业集成（2026-08-04，真机全链收官）

真机验收（OpenJDK 21.0.11 + Spring Boot 3.3.4 + 真 deepseek，**全程没手工起 Rust**）：
Java 网关自己 `ProcessBuilder` 拉起 Rust 子进程 → 经 **ready-file** 拿到 OS 分配的端口
（`--port 0` → 49611，**不解析 stderr**，实现用 `hard_link` 而非 `rename`：要求目标不存在，
陈旧文件不会被误认为本次启动）→ chatid 幂等（201 `created` / 200 `existing` / 重启后 200
`recovered`）→ `GET .../events` 67 帧、**`id:` 游标保留**、`thinking_delta` 逐帧不缓冲 →
停 Java **Rust 一起干净退出、无孤儿**、会话落盘。

「产生 SSE 而非代理 SSE」的判断兑现：那四个坑（不缓冲/不压缩/超时/取消传播）在这条链上
**结构上不存在**，MVC 也扛得住。五个 issue：059 hub 泄漏（静态分析怀疑 → **实测坐实**
5.02s FAILED → 0.03s ok）/ 055 chatid 幂等（变异测试发现穿越断言**假绿**，加固后抓到真实
战果 `a/etc/passwd.jsonl`）/ 056 拉取端点 / 057 断开检测（整套复用 `SubscriberGuard`，
零新取消逻辑）/ 058 网关+生命周期。

**一条要记住的观察**：跨进程重启后拉 SSE 拿不到旧帧——恢复的是**消息历史**（store，随
`.jsonl` 落盘），不是**事件流**（ring 是内存重放缓冲，从不持久化）。`Last-Event-ID` 补发
只在同一进程生命周期内有效。这不是缺陷，但极易被误读成缺陷。

### 旧计划段（历史，勿删——排期依据）

真实提问驱动的三条（「java 直接调用哪个库」→「这样启停不受 java 控制？」→「sse 再加一种
输出，何必一定要 sse」→「java 跟客户端是 sse 的」）。定性：**SSE 保留在 Java→浏览器那一跳**
（产生 SSE 是标准做法），**Java→Rust 换拉取式**（代理 SSE 才有那四个坑 + 强制 WebFlux）。
接缝见 [INTEGRATION.md](INTEGRATION.md)、决策 25。五个 issue：**059 hub 泄漏（排最前，被
chatid 放大）** / 055 chatid 幂等 getOrCreate / 056 拉取式端点 / 057 拉取式断开检测 /
058 Java 网关升级 + 真机 dogfood（终点）。

设计期勘查捞到一条**既存缺陷**单列 059：`SseHub` 自持有 `handle`（内含 `broadcast::Sender`）
而 drain 任务持有 `Arc<SseHub>`——它等的 `recv() == None` 被它自己拿着的 Sender 挡住，于是
session 死后 drain 不退出、全 crate 唯一的 `hubs.remove(&id)` 永远执行不到，每个死会话泄漏
一个 hub + 一个挂起的 task。**静态分析结论、尚未实测**，所以 059 第一步是「先写会红的测试」，
查明不存在也算有效产出。

### 已完成：M8 模型侧异步编排（2026-08-04，真机 dogfood 收官）

真机验收（deepseek 真实上游 + curl 直打，不经浏览器）：模型自发 `spawn(background)`×3 →
`status` 观测 → `collect`×3 → 汇总。**决定性证据是树快照第 141 帧：`root:Thinking` + 三个子
同时 `Thinking`**——阻塞 spawn 下 root 必然卡在 `ToolsPending` 直到子收敛，这一帧结构上不可能，
它就是 M8 相对 M7 的全部增量。含后台子的 turn `/undo` → 21 条 entry 连带整棵子树退干净
（`turn_id` 继承 + `ToolsAllowed→Null` 对后台子同样成立，**一行新代码都没写**——决策 24
「子 agent 不跨 turn」换来的正是这个）。

四个 issue：051 `status`（红线 11 字节确定，删 `sort_by` 就红）/ 052 `spawn(background)` +
孤儿收尾（三条闸各做突变验证；**静止条件一个字没改**——后台子的 provider 调用本就住 `calls` 里）/
053 `collect`（前置重构量出三刀：`dispatch.rs` 293→181、抽 `reply.rs`、拆 `child_outcome.rs`；
逮到 052 留的真 bug：领了还报「没人领」；红线 6 对抗测试用**诱饵子 agent** 推世代，
排除「没落地」的第二种解释）/ 054 专属 `OrphanedChild` 变体 + 面板**零代码**（活树是纯派生读，
后台子在 store 里跟别的 agent 没区别）+ 真机。

### 已完成：M8 的旧计划段（历史，勿删——排期依据）

真实反馈驱动：「子 agent 不该是黑盒」——M7 给了**人看**的活树；用户追问「模型自己要不要
工具去获取子 agent」——这是**模型看**的对偶。选**大版本**：给模型 `spawn(background)` /
`status` / `collect` 三个工具，让它**中途观测子 agent 并改变编排**（不是加并行——并行 spawn
早有；多出的是「看得到、反应得了」）。**关键决策 24：子 agent 仍不跨 turn**（turn 内异步），
避开跨-`run_turn` 映射/undo 重写/per-child 取消三座大山，红线全不破，决策 20 前台 spawn 一行
不改。接缝见 [ORCHESTRATION.md](ORCHESTRATION.md)。四个 issue：051 `status`（独立可先发）/
052 `spawn(background)`+孤儿取消（opus，碰 pump 不变量+红线 6）/ 053 `collect`（opus，红线 6）/
054 面板呈现+真机 dogfood。「能用」终点：真机上模型自发 spawn 后台子→status 观测→collect，
面板同时显示多个后台子在跑。

### 已完成：M5 skills 装载（2026-08-03）

放 skill 进 ./skills/ → 模型经常驻索引自己发现并激活 → 用上它带的工具 → undo 连
激活一起退（journaled 白拿）。三家注入分策由 038 探针实测钉死（Kimi/GLM 消息级免费、
DeepSeek 改 system 段尾保 91%——插新消息 120x 归零）。真机 dogfood：模型激活
commit-cn skill 给 039 自己写了提交信息。你最初「前后端都可以 tool+skills+mcp」
里的 skills 补齐；mcp 见上（M6 进行中）。

### 已完成：M4 全部 3 个 issue（2026-08-02）——四里程碑收官

`agent-desktop.app`（+dmg）真机起窗：内嵌同一个 `agent-server` 库、托管同一套
`packages/web` 构建产物（逐文件 SHA256 相同——「前端一套不变」是哈希不是口号）、
真实对话与 undo 经内嵌 server 全通。`agent-server-bin` 独立宿主（bootstrap 提库、
优雅关闭、sessions-dir 自动落盘——顺带修了 Jsonl 缺目录静默失败的暗雷）。
`examples/java-gateway` 参考实现（WebFlux 流式透传三件事；当前 `mvn -q package` 已通过，
后续 M9 继续补 Java 托管 Rust core 的验收）。

### 已完成：M3 全部 8 个 issue（2026-08-02）

真浏览器四幕验收全过（Playwright 驱动，deepseek 真实上游）：①流式对话 +
GuardReport 逐轮可见；②模型第一次 spawn 传错参数被 is_error 拒绝后**自我修正
重试**（决策 20 的「让模型自己收敛」真实上演），两子 agent 帧交错、归属分栏；
③undo 撞 shell 屏障，确认弹层带工具名与 call_id，force 越过；④关页 8s 后
在飞请求被 server 取消（SSE 重放见 Failed:Cancelled）。已知客户端级缺口如实记：
整页刷新开新会话（session id 未入 URL；Last-Event-ID 补发本身经协议级独测钉死）。
crate/包新增：agent-server（actor + HTTP/SSE）、packages/protocol（TS 从 Rust
生成、一致性测试锁死）、packages/web。终局记录在 [034](issues/034-server-multiagent.md)
与各 issue 合并记录。

### 已完成：M2 全部 12 个 issue（2026-08-02）

`cargo run -p agent-cli -- --session s.jsonl`：`/undo` 是 prompt 级真回滚（被撤的轮
在模型记忆里不存在）；越过 `shell/exec` 停下问、`/undo!` 显式越过；**连续两次
kill -9 重启会话都在**、undo 栈跨重启可用。状态机制两句口号兑现：完整状态 =
9 个 primitive 槽位（`Session::primitives()`），恢复 = 快照 + `apply_next`
字面同一个函数。终局验收在 [027 实做记录](issues/027-cli-undo.md)。
workspace 691 测试 / clippy 零告警 / 红线过。新增 crate：agent-store
（fork + 泛型化 + history/snapshot/persist 全家）。旧 TurnState 路已退役，
行为规格只有 Session 一份。

### 已完成：M1 全部 14 个 issue（2026-08-01）

`cargo run -p agent-cli`：真实十轮、模型主动调工具读文件回答、三层兜底全程在线、
Ctrl-C 流中取消进程存活。终局验收记录（含两处标准校准问题的如实说明）在
[014 实做记录](issues/014-cli-shell.md)。workspace 422 测试 / clippy 零告警 /
红线检查通过。crate 布局：agent-core（引擎+兜底判读，零 IO）/ agent-providers
（三家 adapter，零 IO）/ agent-transport / agent-tools / agent-runtime / agent-cli。

### 没做的

M3（server/SSE/多 agent）起全部，尚未细化成 issue。按 [issues/](issues/README.md)。

## 三、里程碑

逐条任务在 **[issues/](issues/README.md)**。

**排序原则：每个里程碑都以「能用」结束**，不按架构层自底向上砌墙。按层排的问题是
中间没有一个点能停下来说「这东西能用了」，而且越往后前面所有假设越是没被真实使用
验证过——赌注一直在加大。

| | 结束时你能做什么 | 关键验收 |
|---|---|---|
| **M1** | `cargo run` 跟模型对话，它能调工具读文件并回答 | 连续十轮不出错；第 2 轮起每轮 `cached/prompt ≥ 0.9`；兜底十轮零告警；`Ctrl-C` 取消当前轮而进程还活着 |
| **M2** | 按 undo 退回上一轮；退出重进能接着聊 | undo 后所有派生值一致；撞上 `Irreversible` 会停下问；杀进程重启会话还在 |
| **M3** | 浏览器打开，多人用，子 agent 并行 | 真浏览器连 SSE 拿到流；断开能取消在飞请求（不白烧 token） |
| **M4** | 装成桌面应用；企业能内嵌 | Tauri 内嵌同一个库，前端代码一套不变 |

M1 从零开始，十四个 issue。**第一个能停下来说「能用了」的点不是「类型定完了」，
是 [022](issues/022-first-provider.md)：一家 provider 打通，`cargo run` 能跟模型说上话。**
类型只定到够走通那一轮为止，其余等有真实使用反馈再补。

**M1 阶段刻意不做**：store、undo、持久化、server、子 agent。它们都不是「能跟模型
对话并让它干活」的必要条件，而每提前一个都是在没有真实使用反馈的情况下多押一注。

## 四、跨阶段的未决问题

这些没定，到相应阶段必须先定：

- **压缩与 undo 的窗口对立**（P3）。`ExtensionOnly` 的 provider 上压缩损失 100%，
  被迫压得又晚又狠，单次 `prev` 特别大，cap 100 条下能 undo 回去的窗口就特别短。
  要选一个：compact 视为不可逆边界，还是压缩前的历史单独存一份不受 cap 约束。
- ~~上下文压缩策略自动按能力位选还是显式配置~~ **已被决策 17 化解**：触发条件是
  上下文窗口压力，不看折扣比——省钱式早压在最贵的家永远不划算，在便宜的家省得有限。
  剩下的只是阈值取多少（P3 连同上一条一起定）。
- **`Adjustment` 要不要可配置成硬失败**（P1 先只打印进 CLI 和日志）。
  「必须调 `fs/read`」被降级后继续跑，某些场景比直接报错更糟。等 M1 真实用两周、
  看清哪类调整真的需要硬失败再定——现在全配置化是在猜。
- **要不要单 session 的 token 预算闸**（M1 用完真实数据再定）。`max_turns` 是**轮数**闸，
  但一轮可以很贵（一次 128k 全价重编码）；死循环兜底目前靠 016 的轮数闸（事前）+
  024 第 3 层的花费形态告警（事后），预算闸是潜在的第二道事前闸。
- ~~子 agent 由谁 spawn~~ **已定为决策 20**（2026-08-02，M2 完成后拍板）：模型经
  内置工具 spawn，结构性硬限兜底。落地在 028/029。
- ~~工具结果大小上限~~ **已定为决策 19**（2026-08-01 拍板）：默认 32 KiB、只留头部、
  core 边界截断、标记确定可见。实现在 `agent-core/src/limits.rs`。
- **缓存兜底第 2 层只跟上一次比——已真实撞上**（M1 验收：工具跳背靠背请求命中
  旧镜像的取整值，真阳性告警一条，PROVIDERS.md「缓存写入是异步的」）。缓解：留最近
  N 个镜像，把「恰等于旧镜像取整」判为写入延迟。M2 里做还是随 024 补丁做，开 M2 时定。
- **工具「索引 + 详情按需拿」**（用户 2026-08-04 提出，M10 之后择机）。今天所有工具的
  **完整 schema 从第一轮就全在 prompt 里**；提议改成先给索引，模型要用哪个再来要详情——
  也就是把 skill 的延迟加载（068 已真机验过：模型说不出没激活那个 skill 的口令）**推广到
  所有工具**。
  - **抽象本身已经是本仓的模型**（TOOLS.md §「模型看到的是一张扁平表」）：内置 / MCP /
    宿主注入在模型眼里就是一张表里的名字，`location` 与 `reversibility` 是宿主按名字现算的、
    不进那张表。所以「agent 能调用的能力都看作 tools」不是新抽象，是既有抽象。
  - **障碍是 038 探针的实测数字**：工具表在 prompt 最前面，而「中途改工具数组」在
    **DeepSeek 上归零 120x**（前面每字节都命中也照样清零；Kimi/GLM ~100% 保住）。
    「拿到详情再把工具加进表」正好是最贵的那个方向——红线 11 就是为这个数字存在的。
  - **一个能同时活过三家的形状**：**名字全留在表里**（表一个字节不变、前缀永不断），
    描述压成一行；详情用一个 `describe` 工具按需取，而**详情作为「工具结果」回来**——
    工具结果是**消息尾部追加**，那是每一轮本来就在做的事、是缓存专门为之设计的方向，
    不是 system 插入。省的是每一轮那一大坨 schema 的钱。
  - **代价**：模型可能不 describe 就直接调（靠一行描述引导，不是硬保证）。
  - **什么时候值得做**：工具少时是噪音（10 个工具，索引和全量差不了多少钱）。它是个
    **规模功能**——宿主注入 50、200 个业务工具时才是「prompt 能不能用」的差别，
    而那恰恰是 M10 的前提。要做就单开一个里程碑，它动的是 prompt 组装的核心。
  - **M15 已落其中一半**（2026-08-11，决策 27）：通用机制（调用时机 + 「详情走
    工具结果」的通道）以 skills 为第一个用户落地，**真机已验**：正文走 tool result
    后 DeepSeek 十轮 cached/prompt 全部 ≥ 97.8%（含 read 发生的轮），段尾税消失。
    「所有工具的 schema 索引化 + `describe`」仍未决——等宿主注入的工具到规模再开，
    届时通道是现成的。
- ~~宿主声明不了 timed 工具~~ **已定为决策 31**（2026-08-12，用户拍板「hooks 机制
  很重要」提前开工）：声明**内容**不声明执行体——`capabilities.prefix` 开局块，
  装配期合成常量文本 timed 工具，校验（`web:`/`desk:` 前缀即撞名判据）与恢复语义
  （`Slot::HostPrefix` 照 073）连着一起定了，没有只加字段。远程执行的路仍然封着、
  TurnEnd 不进协议（宿主墙外天然观测），理由进决策 31。落地 M17（154–158）。
- **要不要 `AgentStart` 第四时机**（子 agent 出生时跑自己的初始化工具；M15 讨论
  记录，2026-08-12）。今天没有用户。真要做是**新时机**不是 `SessionStart` 加
  旗子——执行点（spawn 路径 vs 会话创建）、产物落点（per-agent 状态 vs 会话级
  前缀块）、姊妹缓存含义全不同。决策 28 的「时机管执行、可见性管谁读、两者
  正交」是当时的判据，重开讨论先读它。
- ~~子 agent 上限的配置面开在哪一层~~ **已定为决策 32**（2026-08-12，159 拍板）：
  进程级启动参数，协议面零改动；per-session 与进 store 两案的否决理由进决策 32。
  校验取严（解析失败/`0` 拒绝启动，下限钉 1）。落地 M18（159–163）。
  - **附带修掉一处静默失配**（[160](issues/160-recover-limits-param.md)，已完成）：
    `command/restore.rs` 恢复时硬写 `AgentLimits::default()`，而
    `agent_runtime::recover` 只给了 `history_cap` 入参、没给 `limits`——那句「宿主
    载入后重调」名存实亡。`actor/body.rs` 里「恢复出来的会话带着它自己持久化过的
    配置」这句话**对 `limits` 和 `history_cap` 都不成立**（两者都不持久化，都靠
    宿主再说一遍），注释已一并更正。今天配置值恒等于 default 所以看不出来；上限
    一可配，第一次重启就显形（闸退回 8，而工具描述里还写着配置值）。

## 五、这份文档怎么维护

阶段完成时把它从「未完成」挪进「已完成」，并把该阶段暴露出的新决策补进第一节。
**未决问题解决后要写明结论和理由**，不要直接删——理由比结论有用，半年后重新讨论时
省一轮。
