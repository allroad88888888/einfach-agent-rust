# 状态模型

本仓的核心。undo / redo / 崩溃恢复 / 审计回放是这一套机制的四个投影。

## 为什么这套机制能成立

传统 agent 框架的状态散在对象字段、闭包捕获、临时变量里——你根本说不清「完整状态」
是什么，所以持久化只能靠人肉挑字段，挑漏一个就是一类恢复 bug。

这里能说清：**完整状态 = 所有 primitive atom 的值**。derived 全部可重算。于是

- 快照 = 序列化所有 primitive atom
- 恢复 = 重建 atom 图 + 灌回 primitive 值 + derived 自动重算
- undo 记录量正比于**源状态**，不是全部状态
- 回滚后所有派生值自动一致——不会出现「消息历史回滚了但 token 计数没跟上」

最后一条是重点。手写状态机时那种「回滚漏了一个字段」的 bug，在这里结构上不存在。

## 原子图

**每个槽位都是 family，key 是 `AgentId`。** 没有单例 atom——root agent 只是 id 为
`root` 的那一个，不走特殊路径。整棵 agent 树活在同一个 store 里，见 §「子 agent」。

### Source atoms（primitive，进 undo log）

| slot | 说明 |
|---|---|
| `config` | model / temperature / max_tokens |
| `messages` | 消息历史 |
| `system_base` | 基础 system prompt |
| `skills_active` | 当前激活的 skill id 列表 |
| `tools_registry_version` | u64，registry 变更时 bump |
| `turn_status` | Idle / Thinking / ToolsPending / Done / Error |
| `toolcall.<id>.result` | 在飞时持 `Pending`，key 是 `(AgentId, ToolCallId)` |

`tools_registry_version` 只放一个版本号，registry 本体在 store 外。原因见红线 5：
`store.get()` 返回 owned 值，每次读都 clone，整个工具表放进 atom 会被反复抄。

### Derived atoms（不进 log，undo 后由引擎重放）

```
prompt.system     = f(system_base, skills_active, tools_registry_version)
prompt.payload    = f(prompt.system, messages, config)
turn.pending      = f(本 agent 在飞的 toolcall + 所有子 agent 的 turn_status)
turn.can_submit   = f(turn_status, turn.pending)
ui.token_estimate = f(prompt.payload)
ui.timeline       = f(messages, toolcall results)
```

`turn.pending` 值得单独说：三个 tool 在飞，两个前端一个桌面，`Pending` 沿依赖图自动
汇聚上来。**不要手写「还剩几个没回来」的计数器**——那个计数器就是 undo 之后最容易
对不上的东西。

### Pending 的来历

上游 einfach-core 有个 `#BUSY!` 机制：异步公式在飞时 cell 持 `Value::Error(Busy)`，
沿错误短路把 pending 传播给整条下游，host 结算后写回、依赖重算。

那正是「tool call 在飞 → 整条下游 UI 变 pending → 结果回来自动刷新」的语义。
fork 时把 Excel 错误码全删了，只保留这一个，改名 `Pending`。

## 写入必须收口

`store.set()` 是裸的，谁都能调。**`agent-core` 只暴露 command API，业务代码禁止直接碰
`store.set`**（红线 2），每次 primitive 写入显式留下 `(AtomKey, prev, next)`。derived 不记录。

这不是「顺手记一下」，是唯一可行解。自动捕获变更需要给每个被追踪的 atom 常驻订阅和基线值，
成本 O(被追踪 atom 数)——而本仓每个 agent 的每个槽位都是 family atom，子 agent 又是动态
增长的，这个成本不成立。上游 TS 的 `createHistory` 踩过并写进了注释，照抄结论。

`store.batch(|s| {…})` 一次 = 一个 undo 步。事务边界直接复用 batch，不另造概念。

## Command log

一条扁平日志，一个游标。**undo 就是弹栈顶**——日志按时间排序，弹掉的是最近发生的那一步，
不管它是哪个 agent 干的。

「只回滚某个 agent 的条目」= 跳过日志中间的条目，而中间条目的 `prev` 是在当时的世界状态下
捕获的，跳着回滚就不成立。那是选择性 undo，另一个量级的问题，本仓不做。

```rust
struct Entry {
    seq: u64,
    turn_id: TurnId,          // 两层粒度靠它分组。由 root agent 分配
    epoch: u64,               // 在飞 effect 的校验凭证
    owner: Option<String>,    // 租户归属，现在留着，以后不用迁 schema
    agent: AgentId,           // 仅用于 UI 时间线显示与审计，不参与 undo 判定
    label: &'static str,      // "append_user_msg" / "tool_result"
    changes: Vec<(AtomKey, Value /*prev*/, Value /*next*/)>,
}
```

- `undo(turn)` —— 从栈顶弹到 `turn_id` 发生变化处（UI 默认粒度）
- `undo(batch)` —— 弹一条（开发者/高级模式，可展开的时间线）
- `redo` —— 反向重放 `next`

`turn_id` **由 root agent 分配，子 agent 的所有 entry 继承所在 root turn 的 turn_id**。
子 agent 不产生新的 turn 边界。于是 `undo(turn)` 一次退回一整个 root turn，连带那一轮里
所有子 agent 的工作——这正是「都在一个 store，undo 回滚整个」应有的语义。

### cap 与分支

日志有上限，**默认 100 条**，溢出从最老一端丢。事务日志式能截断正是它相对快照式的优势：
每条 entry 自带完整逆操作，丢掉最老的不影响剩余条目回滚；快照式必须回溯扫描前序历史才能
找到某个 atom 的上一个值，截断即永久丢失。

游标不在栈顶时写入新 entry，**默认覆盖 redo 尾**（丢弃下标 >= cursor 的条目）。
从历史点开分支是显式操作，不是默认行为。

## 落盘的键必须是 AtomKey

`AtomId` 是自增 u64（`inner.next_id += 1`），完全依赖创建顺序。快照存 `(AtomId, Value)`
的话，只要有人往构图函数中间插一行 `create_atom`，所有旧快照的值就整体错位——**而且
不报错，是静默错位**。这是红线 4。

```rust
enum AtomKey {
    Agent(AgentId, Slot),
    ToolCall(AgentId, ToolCallId, ToolCallSlot),   // Request | Result
}
```

只有两个变体。**没有 `Skill(SkillId)`**——skill 的内容在 store 外的 registry 里，
store 里只有「哪些被激活」，那是 `Agent(_, Slot::SkillsActive)`。

`ToolCallSlot::Request` 存一次调用发起时的快照，含**发起当时**的 `Location` 和
`Reversibility`。恢复时必须按发起时的语义决策，不能从可能已经变过的工具表现查——否则一个
当时标为 `Irreversible` 的调用可能被当成 `Pure` 重发。

（类型随相应 issue 落地——主线代码目前为空，见 [ROADMAP §二](ROADMAP.md)。）

`Slot` 定位「怎么还原」，`AgentId` 定位「还原哪一个」。这和上游 TS `createHistory` 的
`HistoryOp { key, scope }` 是同一个形状——那边已经跑过一遍，照抄。

快照 = `Vec<(AtomKey, Value)>`，只存 primitive。`Entry.changes` 落盘时同样用 `AtomKey`。

顺带白拿 schema 演进：新增 slot 在旧快照里找不到 key，用默认值；删掉的 slot 在快照里
是多余项，忽略加一条 warn。不需要写迁移脚本。

`AtomFamily` 本来就是 `K → AtomId` 的映射，这条是顺着它已有的设计往上长。

## 不可序列化的东西挡在 primitive 之外

在飞的 HTTP stream、MCP 的 stdio 子进程、SSE 的 sender、tool 执行的 `JoinHandle`
——这些不是状态，是状态的**执行现场**。

规则（红线 3）：**primitive atom 的值必须全部可序列化。** 活对象放在 store 外面的
runtime registry 里，atom 里只放一个可序列化的句柄引用它：

```
atom:      toolcall.<uuid>.result = Pending       可序列化
registry:  uuid → JoinHandle / oneshot sender     不进快照，重建
```

`AgentValue` 因此**不提供** `Opaque(Arc<dyn Any>)` 这类变体。给了就一定有人塞，
然后快照就有洞，而且是等到恢复时才发现的洞。

## Epoch

一个 tool call 在飞（atom 持 `Pending`）时用户按了 undo，结果回来会写进一个已经被回滚
掉的世界。所以（红线 6）：

- undo 时 bump session epoch
- 每个 effect 发出时带上当时的 epoch
- 回写前比对，不等就丢弃并取消

不做这条后面必炸，而且是偶发、难复现的那种炸。

## 子 agent

**整棵 agent 树活在同一个 store 里**，靠 family 的 `AgentId` 区分实例。不是每个 agent
一个 store。这换来三件分 store 做不到的事：

1. 子读父 = 一次 `get`，走依赖图自动追踪自动失效，不需要任何消息传递机制。
2. 「等所有子 agent 完成」是一个 derived atom，`Pending` 沿图自动汇聚——复用
   `turn.pending`，不用写调度器。
3. **跨 agent 的 undo 天生一致**：父 agent 回滚一步，那一步 spawn 的子 agent 状态在同一条
   command log 里，一起回滚。分 store 的话这是分布式事务。

### AgentId 用路径编码

`root/a1/a1.2`。祖先/后代判断是前缀匹配，不读 store 就能算。

不要用「parent 指针存在 atom 里」——那样读取边界的判定就依赖了 store 状态，而 undo 正在
回滚 store 状态，会绕成死结。

### 读取边界：只允许上下，禁止横读

依赖图因此恒为树。API 只有两个，没有第三个：

```rust
fn read_ancestor(&self, slot: Slot) -> Value;             // 往上：messages / config / skills
fn read_descendant(&self, id: AgentId, slot: Slot) -> Value; // 往下：status / result / usage
```

两个方向可读的 slot 集合**不相交**，加上图恒为树，环在结构上不可能——不靠运行时的
`CyclicRef` 兜底。兄弟之间要交换数据，经共同祖先中转。

### evict 与 undo

**019 实测钉住的三条硬约束**（写进「状态搬进原子图」的设计输入）：

1. **逐出自叶向根**：`destroy_atom` 有反向边直接 panic、`family.evict` 有下游拒绝
   ——引擎写死的顺序，先销 derived 再销 primitive。
2. **逐出状态驱动**：「移出活名单 → 汇聚 derived 重算 → 边消失 → 才可逐出」，
   不能计时器随手 evict。
3. **重建保证 atom 回来，不保证值回来**：逐出不产生 `Change`，undo 只灌条目里带的值。
   despawn 的 teardown command 必须把活值记成 `prev`，否则 undo 拿回默认值
   ——链通、值错、不报错。


子 agent 是短命的，一个 root 会话可能 spawn 上百个，atom 不回收就是泄漏。但结束后 evict
掉它的 atom，用户再 undo 回到它运行中的那一刻，目标就没了。

解法是 `AtomKey` 的又一个红利：**undo / redo 路径遇到不存在的 atom 就按需重建**
（用默认值 create，再灌 `prev`）。上游 TS 的 applier 里 `resolve(op.scope)` 就是 family 的
get-or-create，已经这么干。这条 lazily-recreate 路径必须写进 applier，漏了就是「undo 到
一半发现回不去」。

### 并发

**子 agent 的并发是 IO 并发，不是状态并发。** LLM 调用和 tool 执行在 IO 线程池上并发跑，
回写必须回到 actor 线程串行——和 `tool_result` 走同一条回写路径，不新增机制。

session 边界因此很自然：**一个 root agent + 它的整棵子树 = 一个 session = 一个 actor
线程 = 一个 store**。跨 root 不共享 store。

### 汇聚 atom 的复杂度

「读所有子 agent 状态」的 derived 是 O(子 agent 数)，任一子 agent 变就重算。能短路的走
`Pending` 短路（读到第一个 `Pending` 就返回，不用读完）；不能短路的汇聚（如总 token 数）
要么接受 O(n)，要么做成增量。写这类 atom 时明确选一个，别默认。

## 持久化

### 接口

```rust
trait SessionStore {
    fn append(&self, id: SessionId, entry: &Entry);
    fn drop_oldest(&self, id: SessionId, count: usize);   // cap 溢出
    fn drop_after(&self, id: SessionId, cursor: usize);   // 新分支覆盖 redo 尾
    fn set_cursor(&self, id: SessionId, cursor: usize);
    fn snapshot(&self, id: SessionId, snap: &Snapshot);
    fn load(&self, id: SessionId) -> Option<(Snapshot, Vec<Entry>, usize)>;
}
```

**写入全部 fire-and-forget，没有返回值。** 失败不回滚内存状态，只经 `on_error` 回调上报
——否则一次 IO 抖动就会让 undo 永久卡死。这是上游 TS 版的教训，直接采纳。

**同步 trait 是刻意的。** actor 是单线程的，写入走 mpsc 扔给一个专门的 IO 线程，
actor 不阻塞，`agent-core` 也不用染上 async。

实现随便插：`Memory`（测试）/ `Jsonl`（文件追加）/ `Sqlite` / `Redis` / `Postgres` /
企业自己的。可以分层选：快照和日志用不同后端，甚至 per-session 选——临时会话 `Memory`，
重要会话落盘。构造 session 时传哪个 `Arc<dyn SessionStore>` 的事。

首批实现 `Memory` + `Jsonl`。

### 恢复 = redo

载入最近快照 → 把之后的 `Entry` 按 `next` 一路往前推。**那就是 redo 的循环，同一个
函数**，不写第二套加载逻辑。

这是「derived 必须纯函数」（红线 1）的根据：重放要能得出同样的结果。read fn 里读时钟、
取随机数、读全局可变量，恢复后的派生值就和崩溃前不一样，而且不报错。

### 中断语义

状态恢复是简单的，难的是恢复时那些在飞的东西怎么算。答案复用 `ToolDescriptor.reversibility`
——和 undo 撞上不可逆操作时是同一套判断：

| 崩溃时的状态 | 恢复策略 |
|---|---|
| tool call 在飞，`Pure` | 直接重发 |
| tool call 在飞，`Reversible` | 先跑补偿动作再重发 |
| tool call 在飞，`Irreversible` | **不能重发**，标记 `Unknown`，问用户「这个操作可能已经执行过了」 |
| LLM 流生成到一半 | 整个 turn 回滚 —— 就是 `undo(turn)`，同一个函数 |
| MCP 连接、SSE sender | 不进快照，重连即可 |

倒数第二行：**未完成的 turn 用 turn 粒度的 undo 直接抹掉**。两层 undo 粒度里的 turn
层，在这里正好是崩溃恢复的原子性边界——不是巧合，是同一个概念。

## 消息历史用持久化向量

`messages` 槽位 **不要**用 `Arc<Vec<Message>>`。append 要 `make_mut`，每次 clone
整个 Vec；一千条消息的会话每 turn append 几次就是 O(n) 反复抄。

用 `imbl::Vector`（`im` 停更了，`imbl` 是维护中的 fork）：append 是 O(log n)
且结构共享，undo 日志里存旧版本几乎零成本
——正好是这套 undo 设计想要的。`PartialEq` 也能走结构共享的指针快路。

同理，所有可能变大的 `AgentValue` 变体一律 `Arc`（红线 5）。`store.set` 靠 `PartialEq`
判断「变没变」来决定是否传播，`PartialEq` 不走 `ptr_eq` 快路的话，每次写都是一次深比较。

## 白捡的能力

command log 存在之后，不用额外写代码就有：

- 从任意历史点开分支（「当时换个问法会怎样」）
- 把会话回放给别人看
- bug 精确重现
- 审计
