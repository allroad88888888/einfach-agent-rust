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
| 10 | **砍掉 wasm 目标，Tauri 内嵌 server** | 少一个 crate、少一个编译目标、provider 不用维护两套 |
| 11 | **server 不做鉴权 / 日志规范 / 集群** | 企业边缘层，每家规范不同。只读 identity header 不验证，只遵守 W3C `traceparent` |
| 12 | **`agent-server` 是库不是二进制** | 桌面版内嵌它，企业内部服务也内嵌它。只给二进制的话他们只能在外面套代理 |
| 13 | **Java 网关只是参考实现**，不发 Maven、不跟版 | 避免 Spring Boot 2/3 双分支与 JDK 矩阵的长期维护税 |
| 14 | ~~`Capabilities` 是 core 读的唯一接缝~~ **被 17 取代** | 见 17。能力位分支只是 `match provider` 换了层皮 |
| 15 | **请求组装归 adapter，core 只供料** | 组装的每个决策都依赖能力位（工具晚加放哪、skill 注入到哪、thinking 进不进前缀、temperature 能不能改），core 里做只能做成不看能力位的搬运 |
| 16 | **`ProviderRequest` 存在的理由是线程边界，不是组装** | store 是 `Rc<RefCell>` 不 `Send`，HTTP 在别的线程。必须有一份「在 actor 线程上提取、能带走」的东西 |
| 17 | **core 里不许有任何模型相关的判断**（红线 12）：从「事前问能力」改成「事后报调整」 | core 只说意图，adapter 做不到就报一条 `Adjustment`（encode 时产生，宿主随 `ProviderDone` 事件喂进 loop）。事前分支 N 位就是 2^N 种组合、多数没跑过、加一家要改 core；事后报调整是可见可审计的，加 provider 不动 core，测试组合掉回 1 |
| 18 | **压缩三分**：触发在 core（当前 tokens vs `SessionConfig` 的窗口大小，纯算术——红线 12 禁分支不禁参数）；实现在 core（统一一份，压缩是状态变更，走 command 层进 undo log）；压后摆盘在 adapter（前缀树的家能保共享分支，仅扩展的认赔并报 `Adjustment`） | adapter 是纯函数无权改世界——它偷偷压，prompt 和状态对不上，undo / 审计 / 前缀镜像一起断 |
| 20 | **子 agent 由模型经内置工具 spawn**（006 拍板）：`spawn_agent` 是 Server 工具，spawn 即 tool call 进日志，「等子树完成」= 该槽位收敛，结果以 tool_result 回父 | ①undo/审计免费——走既有 ToolCall 机制，turn_id 继承让「撤一轮连带子树」天然成立；B 路要为编排动作另发明记账路（第二真值来源）②与开山原则一致：AI 决定调用哪个工具，分解只是又一个工具 ③A 不封死 B（编排层=另一个会调 spawn 的调用方），反向不成立。成本兜底：深度≤3/子数≤8/子树轮预算全是参数，超限 = is_error 的 tool_result 让模型自己收敛 |
| 19 | **工具结果上限：默认 32 KiB、只留头部、core 边界截断、标记确定可见** | ≈8k 英文 token，一次调用最多吃 128k 窗口的 ~8%；`fs/read` 有行范围可分次拿。executor 不知道 prompt 预算所以在 core 截；标记进 prompt 必须逐字节确定（红线 11），写明原始大小与「缩小范围重调」指引。头尾各半到 020（shell）再议 |

## 二、现状

### 仓库里现在有什么

```
crates/                   六个 crate（M1 产物，见下）
probes/                   两个探针 + PROVIDERS.md（三家差异的唯一结论文档）+ 原始观测
docs/                     决策、状态模型、工具模型、适配层接缝、红线 12 条、issues
scripts/                  check-invariants.sh（PostToolUse hook + CI）
providers.example.toml    key 模板（providers.toml 已 gitignore）
```

历史注脚：M1 开工前曾整仓清空过一次——那三个抢跑写的 crate 没有 issue、没有独测、
验收事后补，整体删除按流程重写（教训在 [WORKFLOW.md](WORKFLOW.md) §四）。重写后的
版本经独立测试 agent 与真实调用双重验收，质量差异见各 issue 的实做记录。

### 已完成：M4 全部 3 个 issue（2026-08-02）——四里程碑收官

`agent-desktop.app`（+dmg）真机起窗：内嵌同一个 `agent-server` 库、托管同一套
`packages/web` 构建产物（逐文件 SHA256 相同——「前端一套不变」是哈希不是口号）、
真实对话与 undo 经内嵌 server 全通。`agent-server-bin` 独立宿主（bootstrap 提库、
优雅关闭、sessions-dir 自动落盘——顺带修了 Jsonl 缺目录静默失败的暗雷）。
`examples/java-gateway` 参考实现（WebFlux 流式透传三件事；本机无 JDK，
构建验证缺席在 README 显著声明——参考实现的诚实边界）。

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

## 五、这份文档怎么维护

阶段完成时把它从「未完成」挪进「已完成」，并把该阶段暴露出的新决策补进第一节。
**未决问题解决后要写明结论和理由**，不要直接删——理由比结论有用，半年后重新讨论时
省一轮。
