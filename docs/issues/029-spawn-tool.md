# 029 `spawn_agent` 工具与子树驱动

**里程碑** M3 · **依赖** 028 · **模型** opus · **独立测试 agent** ✅ · **状态** 完成

## 目标

决策 20 落地：模型调 `srv:agent/spawn` 分解任务，子 agent 并行干活，结果以
tool_result 回父，undo 一轮连带子树——M3 验收「模型真的分解任务」那半句。

## 做什么

### 1. spawn 工具（runner 内截获，不进 ToolExecutor）

- `spawn_spec()`：`{ "task": string 必填, "tools": [string] 可选（缺省=父的工具子集）}`；
  描述写给模型看（何时该分解、上限是多少）。`Location::Server`、
  `Reversibility::Reversible`（补偿=despawn；子干的不可逆事自带屏障位，组合天然成立）
- runner 收到 `ExecuteTool{tool: "srv:agent/spawn"}` → **截获**：`Session::spawn_child`
  （028 的命令，上限校验在内）→ 子的槽位 = spawn 时快照（task 进子的首条 user 消息，
  tools 进 `ToolsAllowed`）→ 父的该 tool slot 保持 Pending 直到子终态
- 超限 → `is_error` 的 tool_result（决策 20：让模型看到自己收敛）

### 2. runner 事件泵（并行的形状）

`run_turn` 从「单 agent 轮同步」升级为**事件泵**：effects → IO 分派（provider 调用
每 agent 一个 IO 线程、本地工具同步、spawn 截获）→ 事件经统一 mpsc 汇回 →
`session.step(event)` 按事件里的 agent 路由 → 循环，直到 **root 终态且无在飞**。

- **公开签名 `run_turn(&mut Session, &mut RunnerCtx, input) -> TurnStatus` 不变**
  ——agent-server 的 actor 接在上面（030），泵是内部形状
- 子 agent 的 provider 调用**并行**（各自 IO 线程），状态回写全部串行过泵
  （STATE-MODEL：子 agent 的并发是 IO 并发不是状态并发）
- 子终态 → 泵把子的最后 assistant 文本（或 Failed 摘要）作为父那个 spawn 槽的
  tool_result 喂回（`is_error` = 子 Failed）；部分子失败不中止（003 哲学跨 agent 版）
- Cancel 语义照旧会话级：epoch bump 作废所有 agent 的在飞（028 已让它真能触发）

### 3. 事件归属

`RunnerEvent` 带上 `agent: AgentId`（runner 知道每个事件出自哪个 agent 的 step/流）
——多 agent 输出「谁说的」在宿主层解决，**core 的 `Notice` 不动**（028 的注意事项：
别为这个去改跨 SSE 的公开枚举，那是 031/032 的协议面自己组织的事）。
CLI 打印带 agent 前缀（root 不带，子带短 id）。

## 验收

- 假 SSE 脚本：root 首跳回「spawn 两个子（不同 task）」→ 两子各自完成（脚本可区分
  ——服务器按请求体里的 task 路由响应）→ 父第二跳汇总 → `Done`。断言：两子的
  provider 调用**时间上重叠**（并行证据：脚本让子 A 慢、子 B 快，B 先回但父等到
  A 齐才继续）；消息树完整；`turn_id` 全树一致
- 一子 Failed（脚本回 402）→ 父收到 `is_error` 的 tool_result 照常继续
- 超限 spawn（第 9 个子）→ 模型看到 is_error，loop 不断
- **undo 一轮**：spawn+子工作全回滚（028 已证的机制，这里过真 runner 链路）
- `/undo` 后 re-ask：下一轮 prompt 不含子树内容（假服务器请求体断言）
- Cancel 中途：所有子的在飞流被取消标志斩断，会话 `Failed(Cancelled)`
- CLI 输出可分辨（子 agent 前缀）
- agent-server 的 actor 测试零回归（run_turn 签名未变的证据）

## 注意

红线 4 孪生条款：泵里任何按 agent 汇聚的查询走 `Session` 读口/`AgentId` 现查，
不缓存 AtomId。**per-agent 取料不得做成第三个跨 agent 读 API**（028 点名）：宿主
替子 agent 组 `Ingredients` 读的是「它自己的」槽位——走 `Session` 的 per-agent
读口（messages(agent) 之类，028 已有或本 issue 最小加），不是 read_ancestor。
子 agent 的 system prompt：M3 v1 = 固定模板 + task 文本，skills 装载仍未排期。
「等所有子完成」**不建汇聚 derived**——父的 spawn 槽位收敛就是等待语义
（006 决策记录）；028 留的 StillRead 黑盒缺口因此顺延，如实记录。

## 实做记录（实现 agent，2026-08-02）

### 落地的文件

| 文件 | 行数 | 职责 |
|------|------|------|
| `crates/agent-runtime/src/runner.rs` | 221（+119） | **泵**：`run_turn` 公开签名不变，内部是 A 排空待办 / B 收工判定 / C 截止线扫描 / D 统一 channel 上等一条消息 |
| `crates/agent-runtime/src/dispatch.rs` | 171（新） | 四个 effect 的分派 + `srv:agent/spawn` 截获；`Dispatched` 四态告诉泵接着做什么 |
| `crates/agent-runtime/src/provider_call.rs` | 169（+24） | 拆成 `start`（取料 → encode → 发前第 1 层 → 起线程，**不等**）与 `finish`（终态 → 事件）；`ProviderCall` 是在飞凭据 |
| `crates/agent-runtime/src/io_thread.rs` | 130（+48） | 往泵的**一条**共享 `sync_channel(0)` 发，消息自带 agent tag；`DoneDebt` 保证 panic 也留下一条 `IoMsg::Gone` |
| `crates/agent-runtime/src/spawn_tool.rs` | 190（新） | `SPAWN_TOOL` / `spawn_spec(limits)` / 入参解析 / 提权校验 / `SpawnRefused` → 给模型看的文案 |
| `crates/agent-runtime/src/subagent.rs` | 109（新） | 子 agent 的料：固定 system 模板 + 按 `ToolsAllowed` 过滤的工具表 + 有效工具名清单 |
| `crates/agent-runtime/src/subtree.rs` | 153（新） | spawn 槽位记账：child → (parent, call_id, spawn 时 epoch)；子落终态 → 收敛成 tool_result |
| `crates/agent-runtime/src/event.rs` | 79（+21） | `AgentEvent { agent, event }`——`RunnerEvent` 形状一字未动 |
| `crates/agent-runtime/src/ctx.rs` | 240（+21） | 事件出口收成**一个**字段（带归属），`with_agent_events` 换掉整条；`emit(agent, event)` |
| `crates/agent-runtime/src/tool_table.rs` | 188（+68） | `with_spawn(limits)` / `declares(name)`；spawn 判 `Reversible` |
| `crates/agent-runtime/src/guard.rs` | 35（+6） | `report_success` 收 agent（滚动窗口仍是全树一份） |
| `crates/agent-core/src/command/read.rs` | 237（+54） | **本 issue 在 agent-core 的全部改动**：per-agent 取料口 `messages_of` / `status_of` / `prev_prefix_of` / `tools_allowed_of`，不带参数的四个变成它们在 root 上的特化 |
| `crates/agent-cli/src/print/{mod,events,receipts}.rs` | 24 / 213 / 113 | 打印按职责拆开（事件流状态机 vs 一次性回执），事件流带 agent 前缀 |
| `crates/agent-cli/src/main.rs` | 165（+6） | 工具表 `with_shell().with_spawn(limits)`，回调换 `with_agent_events` |

测试：`tests/subagent_{parallel,failures,ingredients}.rs` 三个新集成测试二进制（6 个用例）
+ `tests/support/routed.rs`（**并发**的、按请求体路由的假 SSE 服务器）
+ `spawn_tool` / `subagent` / `tool_table` / `print::events` 的内联单测 12 个。

**没有动**：`agent-server`、`agent-providers`、`agent-tools`、`agent-store` 的 `src`。
`agent_runtime::run_turn` 的公开签名一个字符没改（030/031 的地基）。

### 泵的最终形状

```
run_turn(session, ctx, input):
  清取消标志；建一条 sync_channel(0)；待办队列塞 UserInput{root}
  loop:
    A  while 待办非空:
         事件 → session.step → persist::sync → 逐个分派 effect
              CallProvider  → provider_call::start，凭据进在飞表（**不等**）
              ExecuteTool   → spawn 截获 / 本地同步执行 → 事件回队列
              CancelInFlight→ 置标志 + **清空待办队列**
              Emit(notice)  → 带上「这批 effect 出自谁的 step」发给宿主
         子 agent 落终态 → 收敛成父那个 spawn 槽的 tool_result → 回队列
    B  在飞表空 → 收工（终态就落快照）
    C  过了截止线的在飞调用 → 注入 Timeout 事件；宿主按了 Ctrl-C 而 root
       自己没有在飞调用 → 替它注入一条 Cancel（只补一次）
    D  在统一 channel 上 recv_timeout(20ms)：
         Delta → 在飞表里还认得这个 agent 才发给宿主，否则丢
         Done  → provider_call::finish → 事件回队列
         Gone  → IO 线程 panic 了 → ProviderFailed(Retryable) 回队列
```

「谁在飞」由**在飞表**回答，不由 channel 的连接状态回答——泵自己握着一份发送端，
`rx` 永远不会断。

### 设计判断

1. **收工条件写成「在飞表空」一个条件，不是「root 终态且无在飞」两个。**
   另一半是它的推论：root 要落终态得先让自己那批工具槽收敛，spawn 槽收敛得等子
   agent 落终态，递归下去——所以 root 终态的时候在飞表必然已经空了。写成两个条件
   等于承认存在「root 已经终态、子树还在跑」的世界，而那个世界里泵该怎么办没有
   答案。真正需要单独说的是另一支（表空了但 root 不是终态），那是 016 裁决过的
   「判了 `ProtocolViolation` 但没落终态」，照旧把控制权交还宿主。

2. **`CallProvider` 拆成起飞/落地，是并行的全部。** 012 那个函数在 `recv_timeout`
   上等到底，多 agent 下这一句会把 root 和两个子 agent 排成队。拆开之后
   `ProviderCall` 装的是「起飞时就定了、落地时才用得上」的东西（第 1 层判读结论、
   预测命中、adjustments、这次请求的前缀镜像）——它们必须是起飞那一刻的值。

3. **一条统一 mpsc，消息自带 agent tag；`Disconnected` 这个信号用 `IoMsg::Gone`
   补回来。** 029 之前每次调用一个 rendezvous channel，「IO 线程 panic 了」表现为
   `recv` 拿到 `Disconnected`。换成共享 channel 之后发送端永远还在别的线程手里，
   那个信号消失了。不补的话，一个 panic 掉的 IO 线程会把一个即刻可判的 bug 拖成
   120 秒超时。补法是 `DoneDebt`：线程从起飞那一刻就欠泵一条终态消息，正常路径
   还 `Done`，panic 路径由 `Drop` 还 `Gone`——两条路都还。

4. **被放弃的调用按 (agent, 在飞表) 认领，认不出就丢。** 超时之后仍然不 join、不
   断连接（`provider_call` 那条事故记录没变），但它接着发来的增量现在会真的到达
   泵。判据跟 `Session::step` 对过期 epoch 的处理同源：过期回执是正常现象，静默
   丢弃，不发通报——每条喊一声只会刷屏。

5. **`RunnerEvent` 外面包一层 `AgentEvent`，不给九个变体各加一个 `agent` 字段。**
   归属是这条事件的**元数据**，不是任何一个变体的载荷。包一层之后「每条事件都有
   归属」是类型事实，而不是「加第十个变体时记得也加上」。顺带的结果是
   `RunnerEvent` 的形状一字未动，`agent-server` 那条 `From<RunnerEvent> for
   SessionEvent` 因此不用被迫改——029 §事件归属写着「别为这个去改跨 SSE 的公开
   枚举，那是 031/032 的协议面自己组织的事」，同一条理由对 `SessionEvent` 成立，
   而 031 此刻正在动那个文件。

6. **事件出口是一个字段，不是「普通回调 + 归属回调」两个。** 两个字段就有「两条都
   设了谁生效」这个必须回答、答什么都不好的问题。`RunnerCtx::new` 收的那条不带
   归属的回调在构造时被包成带归属的（丢掉 agent），`with_agent_events` **替换**
   整条。M2 的宿主一行不用改，多 agent 宿主换一条。

7. **`Effect::Emit(Notice)` 的归属来自「这批 effect 出自谁的 `step`」。** `Notice`
   没有 agent 字段，也没给它加（029 原文）；`Effect::Emit` 也没加——泵手上正好有
   这个事实（它刚把那条事件喂进 `step`），让 core 多存一份就是第二真值源。
   **`agent-core` 因此只多了 per-agent 读口，没有任何类型变化。**

8. **spawn 截获以「工具表里声明了没有」为闸。** 宿主没把 spawn 放进表，模型就看不见
   这个名字；万一它凭空猜出来，那就该跟别的不存在的工具一样落 `unknown_tool`，
   而不是在一个没打算开子 agent 的宿主上凭空长出一棵树。按工具名 match 在宿主侧
   合法——宿主本来就持有工具表，这里没有任何模型相关判断（红线 12 管的是 core）。

9. **`spawn_spec()` 放 `agent-runtime`，不进 `agent-tools` 的 builtin。**
   `builtin_specs()` 里那几个全是 `ToolExecutor::execute` 分发得掉的东西；spawn 要
   改的是会话状态，而 executor 既够不着 `Session` 也够不着泵。塞进去只会得到一个
   「声明在 A、执行在 B、A 那边永远 `unknown_tool`」的分裂形状。

10. **子 agent 的第一条 user 消息走 `Session::step` 的正门。** 刚 `spawn_child` 出来的
    子 agent 十个槽位全是默认值，`Status` 就是 `Idle`，于是转移表 `Idle + UserInput`
    那一格原样接住 task 文本、发出它自己的 `CallProvider`。**「子 agent 怎么开始
    干活」因此没有专门的代码路径**，也没有新的事件/effect 变体。

11. **固定模板不含任务文本，任务只在第一条 user 消息里。** 这不只是形式：模板只依赖
    `AgentLimits`，所有子 agent 的 `[Tools][System]` 前缀因此逐字节相同（红线 11），
    前缀缓存在兄弟之间可以共享；把 task 塞进 system 则每个子 agent 的前缀各不相同，
    每一个都全价。宿主自己的 system 分段原样在前面（子 agent 跟 root 在同一个环境里
    干活，丢掉它只会让子 agent 更笨），固定模板追加在末尾。

12. **子的工具表按宿主表的顺序过滤，不按 `ToolsAllowed` 的顺序拼。** 工具表在 prompt
    最前面，两个子 agent 拿同一份子集却排成两种顺序的话，它们之间的前缀缓存一次也
    命不中（红线 11）。过滤保序天然做到，不需要再排一次。

13. **模型点名了父自己都没有的工具 → 显式拒绝，不静默过滤。** 静默过滤出来的子 agent
    会带着一份跟模型以为的不一样的工具表干活，然后在子 agent 那边报一个跟 spawn
    毫无关系的 `unknown_tool`。拒绝文案里点名缺的是哪几个、父现在有哪些。

14. **spawn 判 `Reversible`。** 补偿动作是 `despawn_child`（028 已实现），这正是这个
    等级的定义。「子 agent 会去干不可逆的事」不构成把 spawn 保守成 `Irreversible`
    的理由：那些事各自带自己的屏障位，而且跟父的 spawn 那条 entry 在**同一条日志、
    同一个 turn_id** 上（决策 5），undo 往回走会先撞上子 agent 那条屏障停下来问，
    轮不到 spawn 这条。反过来判 `Irreversible` 会让「拆了任务的那一轮」一律撤不掉，
    哪怕子 agent 只读了两个文件。

15. **结果回父 = tool_result，`is_error` = 子 `Failed`；`Done { truncated: true }`
    不算失败。** 撞轮数闸的子 agent 手上已经有半份答案，那份答案比一句「失败了」
    有用得多（003 的哲学跨 agent 版），前面加一行固定说明让模型知道它是被截断的。
    取回的是子 agent 最后一条 assistant 消息里的 `Text` 块：`Thinking` 是它的思考
    过程（要不要进 prompt 是 adapter 的判断，不该由我们替父决定），`ToolUse` /
    `ToolResult` 是干活痕迹，父要的是结论。

16. **没有建「等所有子完成」的汇聚 derived**（029 §注意的要求）。父的 spawn 槽位收敛
    **就是**等待语义（006 决策记录）。为同一个问题再准备一个可能对不上的答案，
    正是红线 4 孪生条款最怕的形状。记账表里存的是 `AgentId` / `ToolCallId`，
    一个 `AtomId` 都不缓存——那张表跨越「起飞」和「落地」两个时刻，中间可能夹着
    undo。

17. **spawn 槽记的是 spawn 那一刻的 epoch，不是收敛那一刻的。** 父等的是这一代发出去
    的那次调用；中间被取消/undo 推过世代的话，这条结果就该跟别的在飞回执一样被
    `Session::step` 的闸挡掉（红线 6）。用「现在的 epoch」交差等于绕过闸。

18. **取消要斩三样东西，不是一样。** ①在飞的 HTTP 流（共享取消标志，照旧）；
    ②**待办队列**——「刚 spawn 出来的子 agent 的第一句话」是 `UserInput`，不带
    epoch（用户意图不过闸），不清队列它会绕过取消照常起飞；③**替 root 说一声**：
    029 多了一种 M2 没有的形态——取消发生时 root 处在 `ToolsPending`，自己没有任何
    IO 在飞，那条「取消从流上回来」的路对它不存在。不补的话取消斩掉全部子 agent
    而 root 停在 `ToolsPending` 永远等不到结果（子 agent 的 `Cancel` 已经把世代推走，
    它们的 tool_result 会被 epoch 闸正当地丢掉），`run_turn` 返回一个非终态——用户
    按了 Ctrl-C 却看不到「取消了」。**root 自己在飞时不补**，那条路已经通了，补了
    只会让它收到两次 `Cancel`，第二次落在终态上就是一条没意义的协议违规通报。

19. **第 3 层滚动窗口（`guard_history`）全树共用一份。** 它记的是「这个会话最近几轮的
    缓存命中观测」，而一次会话对 provider 的用量本来就是全树合起来的那一笔账。
    按 agent 分窗会让每个短命子 agent 各自攒一条永远够不到窗口宽度的序列，第 3 层
    对谁都失效。

20. **CLI 的前缀：root 不带，子带路径去掉 root 那一段（`root/a1/a2` → `[a1/a2] `）。**
    不用「最后一段」当短 id：`root/a1/a1` 和 `root/a2/a1` 的最后一段都是 `a1`，
    而那是两个不同的 agent。打印状态机额外记「上一条是谁说的」——并行的子 agent
    会把增量交错吐出来，换人就得先收尾当前这行，否则两句话会拼进同一行。
    `print.rs` 因此顶破 300 行，按职责拆成 `events`（有状态的事件流状态机）/
    `receipts`（无状态的一次性回执文案）。

### 两条明说的代价

1. **子 agent 干完不自动 `despawn`。** 于是 `max_children` 在没有显式回收的宿主上
   退化成「每个 agent 一生 8 个直接子」，第二轮再想拆就会收到那条 `is_error`。
   不自动回收的理由：①despawn 是一条显式命令（019 三约束、状态驱动**会拒绝**），
   把它做成泵的自动副作用，等于给「undo 到一半发现回不去」开一条只有长会话才走得到
   的路；②每个终态子 agent 一条 teardown entry 会把它整段消息历史记成 `prev`，
   而日志 cap 是 100 条——自动回收会让多 agent 会话「能 undo 回去的窗口」急剧缩短
   （ROADMAP §四已经把这一类对立列成未决问题）。**这条限制是可见的、可预期的拒绝**
   （模型收到带数字的 `is_error`），不是静默错值；而且用户手上已经有一条回收路径：
   `/undo` 掉那一轮，子 agent 就不在活名单上了，格子跟着空出来。真要常驻回收，
   `despawn_child` 在 028 就位，接一条 `/despawn` 或者子树预算是后续 issue 的事。
2. **子 agent 不跨 root turn 复用**：每次 spawn 新的。028 推来的第 2 条（「跨 root
   turn 还活着的子 agent 需要一条『重置本 agent 轮状态但不铸新 turn_id』的命令」）
   因此**不需要**——它给的两个选项里选了「干脆每轮 spawn 新的」。

### 与 028「推给 029 的」逐条对账

| 028 推来的 | 029 的处理 |
|---|---|
| 1. `Notice` 没有 agent 归属 | **`Notice` 不动，`RunnerEvent` 也不动**：归属包在 `AgentEvent` 外层，`Effect::Emit` 的归属由泵从「这批 effect 出自谁的 `step`」取。跨 SSE 的公开枚举（`agent_server::SessionEvent`）一字未改——归属进不进协议是 031/032 的判断 |
| 2. 子 agent 没有 `begin_turn` | 选「每轮 spawn 新的」，那条重置命令不需要（见上「两条明说的代价」第 2 条） |
| 3. 取消仍然是会话级 | **保持会话级**，并补了两处会话级取消在多 agent 下的真实缺口（清待办队列、替 root 说一声，判断 18）。按 agent 取消**不做**：真实场景里用户按 Ctrl-C 要的是「这一轮别跑了」，而不是「停掉三号子 agent 但别的接着烧钱」；真需要的时候它是一个新的 `Effect` 变体，不是给现在这个加字段 |
| 4. 取料读口还是 root 专属，**别做成第三个跨 agent 读 API** | 加了四个 per-agent 取料口（`messages_of` / `status_of` / `prev_prefix_of` / `tools_allowed_of`），**不带参数的四个变成它们在 root 上的特化，同一条实现**。它们不是跨 agent 读：宿主替某个 agent 取它自己的槽位不产生图上的边，也没有方向可校验。跨 agent 读仍然只有 `read_ancestor` / `read_descendant` 两个，一行没碰。读口是**非创建**的（走 `peek`，键不在就落 `default_value`）——028 判断 6 对跨读口下的判断，per-agent 取料口没有理由更宽松 |
| 5. 汇聚 derived 一律 family 现查、禁止焊 `AtomId` | **没有建汇聚 derived**（判断 16）。记账表存的是 `AgentId` / `ToolCallId`，查询一律拿 id 现问 `Session`，一个 `AtomId` 都不缓存 |
| 6.（合并记录）`StillRead` 黑盒不可达，等 029 的汇聚读边补 | **顺延，如实记录**：029 决定不建汇聚 derived，那条读边因此仍然不存在，`despawn_child` 的「仍被读依赖 → 拒绝」分支在黑盒层面依然构造不出来（源码级单测 `an_outside_reader_refuses_the_whole_despawn` 照旧覆盖它）。第一个真实触发场景要等到有 UI/审计侧读子 agent 状态的 derived 那一天 |

### 验收对照

| 验收 | 落点 |
|---|---|
| root 首跳 spawn 两个子 → 两子各自完成 → 父第二跳汇总 → `Done` | `tests/subagent_parallel.rs::two_children_run_in_parallel_and_the_parent_waits_for_both` |
| 两子的 provider 调用**时间上重叠**（并行证据） | 同上：`RoutedServer::overlapped("任务A", "任务B")`——每条连接一个线程的假服务器记下每次请求的服务区间 |
| B 先回但父等到 A 齐才继续 | 同上：`b.end < a.end` 且 `父第二跳.start > a.end` |
| 消息树完整；`turn_id` 全树一致 | 同上：root 四条 / 子 agent 两条 / 子的结论逐字进父的 tool_result；`history()` 里每条 entry 的 `turn_id` 都等于 `session.turn_id()`，且 changes 里出现 `root/a1` 的键 |
| 一子 Failed（402）→ 父收到 `is_error` 的 tool_result 照常继续 | `tests/subagent_failures.rs::one_child_failing_becomes_an_is_error_tool_result_and_the_parent_carries_on` |
| 超限 spawn（第 9 个子）→ 模型看到 `is_error`，loop 不断 | `tests/subagent_failures.rs::the_ninth_child_is_refused_...`（九个槽全收敛、只有第九个 `is_error`、文案带数字、`root/a9` 压根没被建出来） |
| **undo 一轮**：spawn + 子工作全回滚 | `tests/subagent_parallel.rs::undoing_the_turn_takes_the_whole_subtree_...`（root 与子的消息都空、子不在活名单上、`UndoReport::Applied` 证明没被屏障挡） |
| `/undo` 后 re-ask：下一轮 prompt 不含子树内容 | 同上：对**假服务器收到的请求体**断言五段痕迹一段都没有 |
| Cancel 中途：所有子的在飞流被斩断，会话 `Failed(Cancelled)` | `tests/subagent_failures.rs::cancelling_mid_turn_cuts_every_child_and_fails_the_session`（含「不是超时抢跑」的时间断言、两个子 agent 各自也是 `Failed(Cancelled)`） |
| CLI 输出可分辨（子 agent 前缀） | `agent-cli/src/print/events.rs::root_has_no_prefix_and_children_are_told_apart` + `tests/subagent_parallel.rs` 里对 `AgentEvent.agent` 的归属断言 |
| 子的工具表 = `ToolsAllowed` 快照过滤后的宿主工具表；固定 system 模板；task 是第一条 user 消息 | `tests/subagent_ingredients.rs`（全部对请求体断言） |
| `run_turn` 签名未变（agent-server actor 测试零回归） | 见下面收工命令输出 |

### 收工命令输出

```
$ cargo test --workspace
passed: 850  failed: 0  ignored: 0
（`test result: FAILED` 出现 0 次）

$ cargo clippy --workspace --all-targets -- -D warnings
clippy exit=0        # warning/error 行数 0

$ bash scripts/check-invariants.sh --all
红线检查通过
规则与理由：docs/INVARIANTS.md
invariants exit=0

$ find crates -name '*.rs' -path '*/src/*' | xargs wc -l | awk '$1>300'
（空 —— 全仓 src 无一顶破 300；本 issue 最大的是 agent-runtime/src/ctx.rs 240）
```

**agent-server 的 7 个 actor 测试零回归**（`run_turn` 签名未变的证据）：

```
a_panicking_provider_kills_only_the_actor_thread_and_registry_reports_it_dead ... ok
cancel_during_an_in_flight_turn_lands_failed_cancelled_within_hundreds_of_ms  ... ok
a_session_closed_and_reopened_recovers_its_history                            ... ok
only_one_concurrent_open_of_the_same_id_succeeds                              ... ok
two_inputs_sent_back_to_back_both_run_and_in_submission_order                 ... ok
two_subscribers_of_the_same_session_get_identical_event_sequences             ... ok
two_sessions_run_concurrently_without_crossing_events                         ... ok
```

**关于 850 这个数**：开工前实测基线 800/0（与 028 合并记录一致）。本 issue 新增
`#[test]` 19 个（`spawn_tool` 5 + `subagent` 2 + `tool_table` 3 + `print::events` 1 +
三个 `subagent_*.rs` 集成用例 6，其余差额来自 `agent-server`——**同一时段并行落地的
031（HTTP/SSE 层）**，本次收工期间它自己还在涨。跟 028 当时的处境一样，总数是移动
靶；可比的硬事实是 **failed 恒为 0**，且 agent-server 的 7 个 actor 测试全绿。

**一处不是本 issue 造成的失败，如实记录**：`cargo test --workspace` 的最后
`Doc-tests agent_server` 编译失败——

```
error[E0365]: `SubscriberGuard` is private, and cannot be re-exported
  --> crates/agent-server/src/http/hub/mod.rs:63:16
```

那个文件是 031 正在写的 HTTP hub（本 issue 一行没碰 `agent-server/src`，开工时
这个目录还不存在）。它不影响任何测试二进制（850 个用例全部跑完且全绿），只让
rustdoc 那一步的 crate 编译不过。留给 031 收。

### 合并记录（主会话）

双侧零分歧：独测 9 覆盖点首跑全过（并行重叠窗、深度链、提权拒绝、兄弟前缀
字节级、取消零新连接、幽灵事件零）。turn_id 一致性用 UndoReport 整轮吞噬做
间接强证（公开面无逐条日志迭代器——如果哪天需要，开 issue 别私开后门）。
「子不自动 despawn」的取舍收（可见拒绝 + /undo 回收路径，真实使用哭了再开
常驻回收 issue）。M3 链 A 完工：模型分解任务、子树并行、undo 连坐——
决策 20 全部兑现。