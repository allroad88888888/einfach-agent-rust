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

`Slot::ALL`（`agent-core/src/graph/slot.rs`）现在是十二个，全部有写入点：

| slot | 说明 |
|---|---|
| `Messages` | 消息历史 |
| `Status` | `Idle` / `Thinking` / `ToolsPending` / `Done{truncated}` / `Failed(_)` |
| `ToolSlots` | 本轮的工具槽，顺序 = 模型请求顺序；每个槽 `Pending` 或 `Finished{content,is_error}` |
| `PrevPrefix` | 上一次请求的前缀镜像（adapter 的比对材料），第一轮之前是 `Null` |
| `NextMessageId` | 下一个要铸的 `MessageId`（从 1 起严格递增） |
| `TurnsUsed` / `MaxTurns` | 本轮已发起的 `CallProvider` 次数与上限 |
| `RetriesUsed` / `MaxRetries` | 当前失败-重试链的连续失败次数与上限 |
| `ToolsAllowed` | **spawn 当时快照的工具子集**，兼**活名单**：`Null` = 这个 agent 不在活名单上 |
| `SkillsActive` | 当前激活的 skill id 列表（排序去重的数组，红线 11） |
| `HostTools` | **宿主建会话时声明的工具**（073）：按名字排序的 `{name, description, schema, reversibility}` 数组。恢复时原样回来，宿主不必重连时再报一遍 |

`ToolsAllowed` 一个槽位身兼两职不是省事：**「这个 agent 是被 spawn 出来的，带着这份工具
子集」**——`Null` 是这个事实的缺席，不是第二个字段。于是「从没 spawn 过」「spawn 被 undo
掉了」「已经 despawn」三种情况在状态上完全一致，因为它们**就是**同一种状态。

**设计里有、至今没有写入点的三个**：`config`（model / temperature / max_tokens）、
`system_base`（基础 system prompt）、`tools_registry_version`（u64，registry 变更时 bump）。
不是漏了，是 026 的裁决：没被真实使用验证过的槽位，跟没写一样，只是它看起来像做完了。
补进来的时候要连 `graph/visibility.rs` 的方向一起显式站队（红线 10 靠那处穷举 match 守）。

`tools_registry_version` 将来也只放一个版本号，registry 本体在 store 外。原因见红线 5：
`store.get()` 返回 owned 值，每次读都 clone，整个工具表放进 atom 会被反复抄。

**在飞的工具不是 per-call 的 atom**：`AtomKey::ToolCall(agent, call_id, Result)` 这个键
存在（落盘键的变体集合不能事后改，所以先留着），但**没有任何生产写入点**——一轮里哪些
工具在飞，活在本 agent 的 `ToolSlots` 那一个槽里（`SlotState::Pending`）。下面
「Pending 的来历」说的语义因此落在槽位数组上，不是落在每个 call 一个 atom 上。

### Derived atoms（不进 log，undo 后由引擎重放）

**今天只有一个落地**（`DerivedKey`，`graph/slot.rs`）：

```
ToolsConverged(agent) = f(本 agent 的 ToolSlots)   // 全都不是 Pending 了吗
```

它的形状是刻意的：**扫槽位，不是维护一个计数器**。计数器是 undo 之后最容易对不上的
东西——回滚了槽位却没回滚计数，收敛条件就永远差一格或早满一格，而且不报错。搬进原子图
之后连「忘了维护」都不可能：这里没有可维护的状态，只有一次重算。未收敛答 `Pending`
而不是 `Bool(false)`，正是为了让它沿依赖图往下游传播。

设计上还该有、**但还没有落地**的几个（写在这儿是为了别让读者以为已经这样了）：

```
prompt.system     = f(system_base, skills_active, tools_registry_version)
prompt.payload    = f(prompt.system, messages, config)
turn.pending      = f(本 agent 在飞的工具槽 + 所有子 agent 的 Status)
turn.can_submit   = f(Status, turn.pending)
ui.token_estimate = f(prompt.payload)
ui.timeline       = f(messages, 工具槽结果)
```

现状：`Ingredients` 由宿主**每一轮现组**（`agent-runtime/src/provider_call.rs` 的 `start`），
skill 正文与它自带的工具由 `ToolTable::skill_injection` 现算。结论（「换一个 skill 不碰
消息序列化」「不要手写还剩几个没回来的计数器」）不变，只是 prompt 组装这一段今天不走
依赖图——ARCHITECTURE 里「料单由引擎增量维护」的说服力目前只在 `messages` 上兑现。

`turn.pending` 落地时值得单独说：三个 tool 在飞，两个前端一个桌面，`Pending` 沿依赖图
自动汇聚上来。跨 agent 的那一半已经有地基了（`ToolsConverged` 就在这个位置汇聚）。

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
// agent-store/src/history/log.rs —— 对 agent 词汇一无所知
pub struct Entry<K, V, M> {
    pub seq: u64,                    // History 铸造，严格递增，不回收
    pub meta: M,                     // 上层填
    pub changes: Vec<Change<K, V>>,  // Change { key, prev, next }，prev 写入前当场捕获
}

// agent-core/src/command/meta.rs —— M 在 agent 侧就是这一份
pub struct EntryMeta {
    pub turn_id: u64,          // 两层粒度靠它分组。由 root agent 分配
    pub epoch: Epoch,          // 这一步发生在哪一代（红线 6 的凭证）
    pub label: &'static str,   // "user_input" / "provider_done" / "tool_result" / …
    pub barrier: bool,         // 不可越过的屏障：这一步记录了一次 Irreversible 工具调用
}
```

**泛型三段式不是花架子**：`History` 住在 `agent-store`，那个 crate 不许 import
`agent-core`（ARCHITECTURE §包结构），所以 `turn_id` / `epoch` / `label` 这些 agent 词汇
整组成为泛型 `M`。落盘时 `M` 是 `PersistedMeta`（`agent-runtime/src/persist/meta.rs`），
字段一一对应，只有 `label` 从 `&'static str` 换成 `String`——进程内的标签是有限的编译期
常量集，落盘的标签是历史数据，允许出现这一版不认识的取值。

`barrier` 是 undo 屏障的**唯一**落盘依据：宿主派发不可逆工具前调 `Session::mark_irreversible`，
随后那条 `tool_result` entry 就带上这一位，`undo` 撞上它返回 `UndoOutcome::Blocked`。
崩溃重启之后仍然拦得住——这一位在文件里。

**没有 `agent` 字段**：每处变更的归属藏在 `Change.key` 里（`AtomKey::agent()`），
一条 entry 可以横跨多个 agent 的键。undo 本来就不看它（一条扁平日志按时间排序），
UI 时间线与审计从 `changes` 里取。

**也没有 `owner`（租户归属）字段。** 曾经写着「现在留着，以后不用迁 schema」——那句是
反的：真要多租户，得往 `EntryMeta` + `PersistedMeta` 加字段，那就是一次落盘 schema 变更
（旧行少一个键，`Deserialize` 会失败，除非同时给 `#[serde(default)]`）。要么现在就加，
要么如实承认将来要迁。

- `undo(turn)` —— 从栈顶弹到 `turn_id` 发生变化处（UI 默认粒度）
- `undo(batch)` —— 弹一条（开发者/高级模式，可展开的时间线）
- `redo` —— 反向重放 `next`

`turn_id` **由 root agent 分配，子 agent 的所有 entry 继承所在 root turn 的 turn_id**。
子 agent 不产生新的 turn 边界。于是 `undo(turn)` 一次退回一整个 root turn，连带那一轮里
所有子 agent 的工作——这正是「都在一个 store，undo 回滚整个」应有的语义。

**⚠️ 在「建会话」那一步写 store 的功能，必须自己 `begin_turn()` 把边界推过去。**
`TurnStatus::Idle` **不是终态**，所以 `handle_input` 对**第一轮**不会自己开新 turn——
建会话时写下的东西会跟用户的第一句话**共用 turn 1**，于是 `/undo` 撤第一句话把它一起撤掉。
073 就踩了这个（宿主注入的工具声明），症状是**静默的**：撤完当场看不出来，要等下次
重开会话才发现工具表少了几个，离现场十万八千里。对刚建好的会话 `begin_turn()` 自己
不产生任何 `Change`（`History::append` 拒绝空步），所以它**不落 entry**，唯一作用就是推边界——
代价为零，别省。

### cap 与分支

日志有上限，**会话层默认 100 条**（`Session::new` 建 `History` 时调一次
`set_cap(Some(DEFAULT_HISTORY_CAP))`），溢出从最老一端丢。注意层次：`History` 结构本身
默认**无上限**（`cap: None`）——它对「一个会话该有多大」一无所知，就像它对 `AtomId`、
`turn_id` 一无所知一样；100 是会话层的策略，不是日志结构的常量。
事务日志式能截断正是它相对快照式的优势：
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
    ToolCall(AgentId, ToolCallId, ToolCallSlot),   // ToolCallSlot 目前只有 Result
}
```

只有两个变体。**没有 `Skill(SkillId)`**——skill 的内容在 store 外的 registry 里，
store 里只有「哪些被激活」，那是 `Agent(_, Slot::SkillsActive)`。

`ToolCallSlot` **今天只有 `Result` 一个变体，而且这一支还没有生产写入点**（在飞的工具槽
活在 `Agent(_, Slot::ToolSlots)` 里）。变体集合先留着，是因为 `AtomKey` 是落盘键的类型：
`Slot` 可以往里加（旧快照缺键用默认值），`AtomKey` 的变体集合不能事后改——两者的稳定性
要求不是一个量级。

设计上还该有一个 `ToolCallSlot::Request`：存一次调用发起时的快照，含**发起当时**的
`Location` 和 `Reversibility`。理由仍然成立——恢复时必须按发起时的语义决策，不能从可能
已经变过的工具表现查，否则一个当时标为 `Irreversible` 的调用可能被当成 `Pure` 重发。
**但它至今没有落地，而且是刻意的**（002 合并时的裁决）：`agent-core` 没有工具表，
现造一份占位快照是编造，一个假的 `Irreversible` 会让 undo 白拦一次 `fs/read`，
正是本仓最怕的静默错值。要补它，得由**持有工具表的宿主**来记。
下面 §「中断语义」上半张表的输入就卡在这一条上。

`Slot` 定位「怎么还原」，`AgentId` 定位「还原哪一个」。这和上游 TS `createHistory` 的
`HistoryOp { key, scope }` 是同一个形状——那边已经跑过一遍，照抄。

快照 = `Snapshot { values: Vec<(AtomKey, AgentValue)> }`，只存 primitive。
`Entry.changes` 落盘时同样用 `AtomKey`。source 槽位**不 lazy 建**，就是为了让「完整状态 =
所有 primitive」在 `Session::primitives()` 那一侧立刻成立——懒建的话，一个从没被写过的
槽位不在 family 里，快照就少一项，恢复时那一项落默认值，碰巧默认值就是它当时的值，
于是永远不报错，直到某天默认值改了。

顺带白拿 schema 演进：新增 slot 在旧快照里找不到 key，用默认值；删掉的 slot 在快照里
是多余项，忽略加一条 warn。不需要写迁移脚本。

`AtomFamily` 本来就是 `K → AtomId` 的映射，这条是顺着它已有的设计往上长。

## 不可序列化的东西挡在 primitive 之外

在飞的 HTTP stream、MCP 的 stdio 子进程、SSE 的 sender、tool 执行的 `JoinHandle`
——这些不是状态，是状态的**执行现场**。

规则（红线 3）：**primitive atom 的值必须全部可序列化。** 活对象放在 store 外面的
runtime registry 里，atom 里只放一个可序列化的句柄引用它：

```
atom:      ToolSlots[i] = Pending                 可序列化
registry:  call_id → JoinHandle / oneshot sender  不进快照，重建
```

`AgentValue` 因此**不提供** `Opaque(Arc<dyn Any>)` 这类变体。给了就一定有人塞，
然后快照就有洞，而且是等到恢复时才发现的洞。

## Epoch

一个 tool call 在飞（工具槽持 `Pending`）时用户按了 undo，结果回来会写进一个已经被回滚
掉的世界。所以（红线 6）：

- undo 时 bump session epoch
- 每个 effect 发出时带上当时的 epoch
- 回写前比对，不等就丢弃并取消

落点是 `Session::step` 的第一道闸：事件带的 epoch 不等于当前世代就**整条丢弃**，
返回空 `Vec`，一个 primitive 都不写（epoch 只增不减，「不等于当前」就等价于「过期」）。
远端工具那一路的凭证由**服务端**保管，客户端 `POST /tool_result` 时既不带也伪造不了 epoch，
只能精确匹配一个仍在等待的 `(agent, call_id)`——见 [TOOLS.md](TOOLS.md) §「回写必须匹配
在飞的调用」。

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
// agent-core/src/command/cross_read.rs
fn read_ancestor  (&self, reader: &AgentId, target: &AgentId, slot: Slot) -> Result<AgentValue, ReadDenied>;
fn read_descendant(&self, reader: &AgentId, target: &AgentId, slot: Slot) -> Result<AgentValue, ReadDenied>;
```

**越界是被显式拒绝，不是返回默认值**——`ReadDenied` 有四个变体：`NotAnAncestor` /
`NotADescendant`（方向不对，横读死在这一条上）、`NotVisible`（方向对了但这个槽位不朝
这个方向开）、`NoSuchAtom`（图上没有这个 atom，**不顺手建一个**）。这一层是红线 10 的
运行时落点；结构面在 `graph/visibility.rs`，那里是**穷举 match，一个 `_` 通配都没有**：
新增槽位不显式站队就编译不过。

今天的两个方向：往上 `Messages`、`SkillsActive`、`HostTools`（子 agent 干活要的上下文；
后两者都是「这个会话有哪些能力」，属于会话不属于某一个 agent）；
往下 `Status`、`ToolsAllowed`（父要知道子干完了没；`ToolsAllowed` 兼活名单，
汇聚 derived 得先知道有哪些活着的子）。其余全是 `Private`——**开放一个方向要有理由，
封闭不需要**。`config` / `system_base` 落地时按语义补进 `Upward`。

两个方向可读的 slot 集合**不相交**，加上图恒为树，环在结构上不可能——不靠运行时的
`CyclicRef` 兜底。（论证：跨 agent 的边只有「后代读祖先的 `Upward` 槽位」和「祖先读后代的
`Downward` 槽位」两种，一个环必须同时含这两种边，于是环上存在某个槽位既被往上读又被往下
读，那要求两集合相交。所以测试断言的是**集合性质本身**，不是几个用例。）
兄弟之间要交换数据，经共同祖先中转。

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
// agent-store/src/persist/mod.rs
pub trait SessionStore<K, V, M> {
    fn append(&self, entry: &Entry<K, V, M>);
    fn drop_oldest(&self, count: usize);                    // cap 溢出，从最老端丢
    fn drop_after(&self, first_seq: u64, count: usize);     // 新分支覆盖 redo 尾
    fn set_cursor(&self, cursor: usize);
    fn snapshot(&self, snap: &Snapshot<K, V>);
    fn load(&self) -> LoadOutcome<K, V, M>;
}
```

**一个实例绑一个会话**，方法上没有 `SessionId` 参数（原 issue 草案里有，实做时去掉了）：
§「子 agent」已经钉死「一个 root agent + 整棵子树 = 一个 session = 一个 actor 线程 =
一个 store」，多会话是「每会话一个 `SessionStore` 实例」，不是「一个实例带 id 路由」——
路由到哪个文件/哪张表是宿主的事，不该是这个端口的事。

**`load` 是三态，不是 `Option`**（027 独测发现，是被否决过的设计）：

```rust
pub enum LoadOutcome<K, V, M> {
    Absent,                              // 这个身份从来没写过东西 → 开新会话是对的
    Refused { reason: String },          // 有会话，但这份数据不能安全加载 → 必须硬失败
    Loaded(LoadedSession<K, V, M>),      // { snapshot, entries, cursor, next_seq }
}
```

`Option` 把「文件不存在」和「有会话但拒绝加载（中部损坏）」压缩成同一个 `None`，
宿主只有一条路可走——当成全新会话，然后**第一张快照就把用户原文件覆盖了**，
损坏之前还能人工修复的数据这下真没了。「没有会话」与「有会话但读不出来」对宿主是两件
完全不同的事，所以它们必须是两个值。`reason` 只带类别与行号一类的诊断信息，
**不带 K/V 内容**——那里面可能是用户对话。

**写入全部 fire-and-forget，没有返回值。** 失败不回滚内存状态，只经 `on_error` 回调上报
——否则一次 IO 抖动就会让 undo 永久卡死。这是上游 TS 版的教训，直接采纳。

**同步 trait 是刻意的。** actor 是单线程的，写入走 mpsc 扔给一个专门的 IO 线程，
actor 不阻塞，`agent-core` 也不用染上 async。

实现随便插：`Memory`（测试）/ `Jsonl`（文件追加）/ `Sqlite` / `Redis` / `Postgres` /
企业自己的。可以分层选：快照和日志用不同后端，甚至 per-session 选——临时会话 `Memory`，
重要会话落盘。构造 session 时传哪个 `Arc<dyn SessionStore>` 的事。

首批实现 `Memory` + `Jsonl`（已落地）。分家的位置是红线 7 定的：`Memory` 跟端口定义
一样零 IO，住在 `agent-store`；真做文件 IO 的 `Jsonl` 住在 `agent-runtime`——
`agent-core` 和 `agent-store` 都被红线 7 禁着，唯一能做 IO 的地方是运行时层。
两个实现共用同一套「游标怎么翻译、snapshot 怎么压实」的逻辑（`SessionLog`）：
分叉一次这套算法，「写→load→重放语义一致」就成了两份各自维护的推导，迟早对不上。

### 恢复 = redo

载入最近快照 → 把之后的 `Entry` 按 `next` 一路往前推。**那就是 redo 的循环，同一个
函数**，不写第二套加载逻辑。

这是「derived 必须纯函数」（红线 1）的根据：重放要能得出同样的结果。read fn 里读时钟、
取随机数、读全局可变量，恢复后的派生值就和崩溃前不一样，而且不报错。

**恢复是忠实重放，不是「用今天的配置重建」**（073 把这句写成了可判定的验收）。宿主在
`POST /sessions` 注入的工具声明因此是**会话状态**（`Slot::HostTools`，建会话时 journaled
写一次），不是每次连上来重报的部署配置：恢复出来的会话带回**它当初那一份**工具表，
而不是宿主今天的新清单——否则历史对话会自相矛盾（模型当初说「我调了 `web:crm/lookup`」，
今天的清单里可能没有它），而且工具表在 prompt 最前面，换一份 = 恢复出来的第一轮前缀
全断（红线 11）。同一条判断也解释了为什么 skill 存的是**激活的 id** 而不是正文：那份
资产在 store 外的 registry 里另有主人，注入的声明在 store 外**没有第二份**。

### 中断语义

状态恢复是简单的，难的是恢复时那些在飞的东西怎么算。设计上的答案复用发起时快照的
`ToolCallRequest.reversibility`——和 undo 撞上不可逆操作时是同一套判断：

| 崩溃时的状态 | 恢复策略 | 今天 |
|---|---|---|
| tool call 在飞，`Pure` | 直接重发 | ⛔ 缺输入 |
| tool call 在飞，`Reversible` | 先跑补偿动作再重发 | ⛔ 缺输入 |
| tool call 在飞，`Irreversible` | **不能重发**，标记 `Unknown`，问用户「这个操作可能已经执行过了」 | ⛔ 缺输入 |
| LLM 流生成到一半 | 整个 turn 回滚 —— 就是 `undo(turn)`，同一个函数 | ✅ |
| MCP 连接、SSE sender | 不进快照，重连即可 | ✅ |

**上半张表现在没有落盘依据**：它要的「发起当时的 `Reversibility`」正是没落地的
`ToolCallSlot::Request`（见上），今天只活在宿主内存里（`RunnerCtx` 的
`PendingRemoteTool`），进程一死就没了。`Session::restore` 只灌快照 + 推 entries，
一个持 `Pending` 的工具槽恢复回来仍然是 `Pending`，而它的执行现场已经不在了。

**这不等于红线 6 或 undo 屏障有洞**——屏障位 `EntryMeta.barrier` 是落盘的，
epoch 恢复后取日志最大值 +1 继续发。有洞的只有崩溃恢复这一条路径。

要补的时候有两条路，**先验证再选，别照着上表直接实现**：① 由持有工具表的宿主记一份
`ToolCallSlot::Request` 快照；② 干脆按下一段那条走——未完成的 turn 整个抹掉，那样上半张
表就是**多余的设计，该删而不是该实现**。先跑一次「tool call 在飞时 kill -9」看现在实际
发生什么（会不会永远卡在 `ToolsPending`），再决定。

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
