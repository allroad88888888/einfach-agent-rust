# 028 多 agent 原子图：路径语义、上下读边界、despawn

**里程碑** M3 · **依赖** 026 · **模型** opus · **独立测试 agent** ✅ · **状态** 完成

## 目标

一个 store 装下整棵 agent 树（决策 3），`Session` 从单 agent 长成多 agent：
子读父是一次 get、等子完成是槽位收敛、跨 agent undo 天然一致——CLAUDE.md
开头承诺的那三件事，在这里成为可测试的事实。

## 做什么

### 1. `AgentId` 补路径语义

`root/a1/a1.2` 路径编码（STATE-MODEL §AgentId）。**`is_ancestor_of` 必须按分隔符
边界判**——`root/a1` 不是 `root/a10` 的祖先，纯前缀匹配是已知坑（M1 前的旧实现
踩过，记录在案）。`child(seq)` / `parent()` / `depth()`；`root()` 语义不变。

### 2. 读边界：只允许上下，禁止横读（红线 10）

- `Slot` 补 `visibility()`：`Upward`（子可读父：Messages/Config——M3 起真实需要的
  那几个）/ `Downward`（父可读子：Status/结果）/ `Private`（其余）。**穷举 match
  无通配**——新增槽位必须显式站队，这是红线 10 的结构面
- `Session` 只暴露两个跨 agent 读口：`read_ancestor(from, slot)` /
  `read_descendant(of, slot)`，各自校验方向与 visibility；**没有第三个 API**。
  兄弟互读在 API 面上不存在，环在结构上不可能

### 3. spawn / despawn 命令（记账走 command 层）

- `Session::spawn_child(parent, child_config) -> AgentId`：构图函数建子 agent 的
  槽位（与 root 同一条 `build_agent` 路径——019 硬约束）、深度/子数上限在此校验
  （数字参数）、写入记账（子的初始槽位值 = spawn 这个 batch 的 changes）。
  `Slot::ToolsAllowed`（spawn 时快照的工具子集）在此新增——**它是本 issue 唯一
  新增的槽位**，029 消费
- `Session::despawn_child(child)`：**019 三条硬约束的第一次真实执行**——
  自叶向根（先 derived 后 primitive；子树递归）、teardown command 把活值记成
  `prev`（undo 回来值才对）、状态驱动（仍被读依赖时拒绝）。despawn 后
  undo → applier 按 K 重建子树、值完整回来（019 的链路第一次跨 agent 跑通）
- `turn_id` 由 root 铸造，子 agent 的 entry 继承（决策 5）——`EntryMeta` 不变，
  铸号点收在 root 的 begin_turn

### 4. `step` 长 agent 维度

`Session::step(agent, event)`——事件与 effect 带的 `agent` 字段（001 就有）开始
真正路由。各 agent 的 turn 状态独立（status/槽位/预算 per-agent），epoch 仍是
会话级（CancelInFlight 的既有语义）。

## 验收

- `is_ancestor_of`：`root/a1` vs `root/a10` 边界用例；自身不是自己的祖先
- 读边界：子读父 Messages ✓；父读子 Status ✓；兄弟读 → API 面不存在（编译期）+
  `read_ancestor`/`read_descendant` 对方向错误的调用显式拒绝；`visibility()`
  穷举且 Upward/Downward 集合不相交（测试断言）
- spawn 记账：spawn 后 history 多一条 entry，changes 含子的初始槽位
- **undo 一轮连带子树**：root 轮内 spawn 子 + 子写状态 → `undo_turn` → 子树
  primitive 全回退（含子 agent 的槽位消失语义——按 019：atom 还在图上但值回 spawn
  前，或 despawn 语义你judge并写明）
- despawn → undo → 子树值完整重建（019 链路跨 agent 版）
- 深度 4 / 子数 9 被拒：`is_error` 语义的错误返回，不 panic
- 单 agent 路径零回归：M2 的全部 `session_*` 测试照绿

## 注意

红线 10 的「环在结构上不可能」依赖两个方向可读的 slot 集合**不相交**——测试要
断言这个集合性质本身，不只测几个用例。红线 4 孪生条款在多 agent 下更要命：
汇聚型 derived（029 会建「等所有子完成」）必须按 AgentId 现查 family。
`Slot::ALL` 驱动的快照遍历要跟着多 agent 走（`primitives()` 现在是全树的）。

## 实做记录（实现 agent，2026-08-02）

### 落地的文件

| 文件 | 行数 | 职责 |
|------|------|------|
| `crates/agent-core/src/ids/agent.rs` | 243 | `AgentId` + 路径代数（`child`/`parent`/`depth`/`is_ancestor_of`/`is_descendant_of`） |
| `crates/agent-core/src/ids.rs` | 67（−23） | 只剩 `ToolCallId` / `MessageId` + `mod agent` 与 re-export |
| `crates/agent-core/src/graph/visibility.rs` | 141 | `Visibility` 与 `Slot::visibility()`（穷举无通配）+ 划分性质的断言 |
| `crates/agent-core/src/graph/slot.rs` | 172（+28） | 新增 `Slot::ToolsAllowed`，`ALL` 9 → 10 |
| `crates/agent-core/src/command/tree.rs` | 130 | 树形查询：`in_session`/`is_live`/`children_of`/`live_agents`/`live_subtree_leaf_first`/`known_agents`/`peek` |
| `crates/agent-core/src/command/spawn.rs` | 271 | `spawn_child` + `ChildConfig` / `AgentLimits` / `SpawnRefused` |
| `crates/agent-core/src/command/despawn.rs` | 289 | `despawn_child` + `DespawnRefused` / `DespawnReport`（019 三约束） |
| `crates/agent-core/src/command/cross_read.rs` | 111 | `read_ancestor` / `read_descendant` / `ReadDenied` |
| `crates/agent-core/src/command/commit.rs` | 82（+24） | `commit_as(agent, ..)`，`commit` 变成它的 root 特化 |
| `crates/agent-core/src/command/txn.rs` | 285（+14） | `Txn::set_key`：按任意逻辑键写（只有 spawn/despawn 用） |
| `crates/agent-core/src/command/step.rs` | 82（+23） | agent 闸：从事件路由 + 活性校验 |
| `crates/agent-core/src/command/undo.rs` | 210（+35） | `rebuild_touched_agents`：应用前把碰到的 agent 的整张图补齐 |
| `crates/agent-core/src/command/restore.rs` | 259（+23） | 恢复建的是整棵树（agent 集合从快照键 + 已生效条目的键里读） |
| `crates/agent-core/src/command/session.rs` | 180（+27） | `limits` 字段；`begin_turn` 是 root 专属的说明 |
| `crates/agent-core/src/engine/event.rs` | 287（+33） | `Event::agent()` 提取器（穷举）+ 它的实检 |
| `crates/agent-core/src/command/meta.rs` | 107（+2） | `KNOWN_LABELS` 加 `spawn_child` / `despawn_child` |

测试：`tests/session_subagent_{spawn,read_boundary,undo,despawn,step_routing,restore}.rs` 六个新文件
+ `despawn.rs` / `spawn.rs` / `ids/agent.rs` / `graph/visibility.rs` 的内联单测。
改了三个存量测试文件的**数字**（槽位 9 → 10）：`session_indep_snapshot_shape.rs`、
`session_state.rs`、`session_indep_accounting.rs` 的穷举 match。

**没有动**：`agent-store/src`、`agent-providers`、`agent-tools`、`agent-runtime`、
`agent-cli`（029 才接线）。`Session::step` 的签名一个字没改——见下面判断 4。

### 裁决：轮内 spawn 的子在 undo 之后是什么

**选「atom 留在图上，值回 spawn 前的默认值」，不是「连 atom 一起 despawn」。**
`ToolsAllowed` 回 `Null` 就是「不在活名单上」，于是 `is_live` 为假、`children_of`
里没有它、`step` 的活性闸把它的事件丢掉——**在语义上它就是没被 spawn 过**。
五条理由，前三条决定性：

1. **逐出可能被拒绝，而 undo 不允许失败。** `AtomFamily::evict` 在有下游/订阅时
   返回 false，`Store::destroy_atom` 有反向边直接 panic（019 实测）。把 undo 建在
   一个会拒绝的操作上，结果是「undo 到一半发现回不去」——正是 019 那条注意警告的
   线上才炸的形状。
2. **逐出不产生 `Change`（019 第 5 条），redo 就无从反演。** undo 若顺手 evict，
   它就做了一件日志里没写的事；redo 只灌 `next`，没有「un-evict」这个动作。
   「redo 能完整回来」这条要求本身就否掉了 B 方案。
3. **applier 的 get-or-create 已经决定了大半**（issue 原文的提示）：`resolve` 只建
   不毁，applier 的实现区一个 `if` 都没有（019 判断 1）。要让 undo 逐出，就得在
   undo 路径上新开一条只有它自己走的分支，而那条分支会和正常路径长期失同步。
4. **`prev` 说的就是这件事**。spawn 那条 entry 的 change 是
   `(Agent(child, ToolsAllowed), prev = Null, next = Json([...]))`——`apply_prev`
   写回 `Null`，「回默认值」不是我们选的，是日志的字面意思。选 B 等于在日志之外
   多做一件事。
5. **「活着」因此是图上的一个值，不是「atom 在不在」**。这让撤销一次 spawn 退化成
   一次普通的值回滚，跟别的 primitive 一视同仁——「跨 agent 的 undo 天生一致」
   这句口号的兑现方式是**没有代码**，不是一段代码。

真正的回收是 `despawn_child` 的职责：它是一条显式命令（记 `prev` + 状态驱动 +
自叶向根），不是 undo 的副作用。对称地，**`undo` 撤销一次 despawn 之后 `redo`
不会重新逐出**——值一模一样（全默认、不在活名单），只有 atom 还占着内存。
日志管值，不管驻留。

### 设计判断

1. **树的形状不存在任何一个 atom 的值里。** 父子关系只在 `AgentId` 的路径上
   （STATE-MODEL 明令不许用 parent 指针 atom：判定读 store，而 undo 正在回滚
   store，会绕成死结）。「有哪些 agent」从 **family 的键空间**读，「谁还活着」
   从 `ToolsAllowed` 读。于是 issue 要求的「唯一新增一个槽位」不是省着用，
   是本来就够——`Children` / `NextChildSeq` 那两个想加的槽位都是第二真值源。
2. **`ToolsAllowed` 一个槽位担两件事，因为它们本来是一件事。** 「这个 agent 是被
   spawn 出来的、带着这份工具子集」，`Null` 是这个事实的缺席。于是「从没 spawn
   过」「spawn 被 undo 撤了」「已经 despawn」三种情况在状态上完全一致——它们
   **就是**同一种状态。root 是唯一例外：它活着而 `ToolsAllowed` 是 `Null`，
   因为它的活性来自会话本身而不是某一次 spawn。
3. **despawn 留下 `ToolsAllowed` 墓碑，不全逐出。** 两个理由，第二个是结构性的：
   ①**号不复用**——铸号取 family 键空间里的最大号，墓碑在号才单调；全逐出的话
   despawn 完再 spawn 会拿回同一个 `AgentId`，审计时间线上出现两个同名 agent，
   而 undo 日志的键正是这个 id。②它**就是 019 第 2 条说的活名单**：029 的汇聚
   derived 要先知道有哪些活着的子，读的就是各个子的 `ToolsAllowed`，那条读边一直
   在，引擎本来也不会让它被逐出。**换句话说墓碑不是我们额外留的，是逐出规则自己
   留下的**——按全逐出写，029 落地的当天 `evict` 就开始返回 false。
   代价：每个死掉的子 agent 残留 1 个 atom 而不是 0 个（十一分之一）。
4. **`step` 的签名没动，路由从事件的 `agent` 字段来。** issue 写的是
   `step(agent, event)`，但事件从 001 起就带 `agent`；再加一个参数就有了两个可能
   互相矛盾的真值源，而且会当场打断 `agent-runtime`/`agent-cli`（本 issue 不许
   动）。路由权仍然没交给宿主：`Session::step` 拿到 `agent` 之后过一道**活性闸**，
   不在活名单上就丢弃。`read.rs` 里那句「effect 的 agent 不从事件里取」的注释
   同步改了——闸的形式从「不看你说的」变成「看，但要核」。
5. **未知/已死 agent 的事件静默丢弃，不发通报。** 和 epoch 闸同源：despawn 之后
   在飞的工具回执陆续到达是**正常现象**，每条喊一声只会刷屏。代价是宿主拼错
   `AgentId` 也静默——但给它开一条会报错的路，就等于给过期回执也开了一条，
   两者在类型上分不开。
6. **两个跨 agent 读口都收 (reader, target)。** issue 写的是
   `read_ancestor(from, slot)`，但同一句里要求「各自校验方向」——只有一个端点就
   没有方向可校验（隐含「父」的话，方向是结构决定的）。两个端点才能把兄弟、自己、
   传反了、别的会话的 id 全部显式拒掉。读口是**非创建**的（`peek`），写入才
   get-or-create：读取有副作用就意味着宿主传错一个 id 会在 family 里静静留下十个
   没人写的 atom，还会跟着进快照。
7. **`Visibility` 的划分测的是集合性质本身**（issue 注意点名）：三类构成 `Slot::ALL`
   的一个划分、两个方向都非空、以及公开面上的形状——对每个槽位，两个读口最多只有
   一个能成功。当前归属：`Upward = {Messages}`，`Downward = {Status, ToolsAllowed}`，
   其余 `Private`。`config`/`skills` 那几个 STATE-MODEL 列过的 Upward 槽位还没有
   写入点（026 的裁决），补进 `Slot` 时穷举 match 会逼它们站队。
8. **`undo`/`redo` 应用之前先 `rebuild_touched_agents`。** applier 的 `resolve` 是
   按**键**的 get-or-create，只建条目里出现过的那几个 atom，不建 derived、也不建
   条目没提到的槽位——多 agent 之后这会让 `primitives()` 少一项，快照跟着少一项，
   恢复时那一项落默认值**而且永远不报错**。补齐放在 applier **之外**：019 推过来
   的账写着「`resolve` 闭包里不要读 store」，而 `build_agent` 末尾要读一次 derived
   把反向边装上。
9. **恢复建的是整棵树。** 「当时有哪些 agent」写在落盘的键上（红线 4 用逻辑键换来
   的红利），不需要另存名单。只建「已生效」那一段涉及的 agent——redo 尾里的 agent
   此刻还不存在，真被 redo 回来时判断 8 那一步会补。不改这里的话，多 agent 会话
   重启后子树整个消失，快照里那些键还会被当成「这一版不认识的键」报上来。
10. **`AgentLimits` 是字段不是槽位**（决策 20 的「数字参数」）。它是会话的**配置**，
    跟 `History` 的 cap 同一类：调大上限不是一次可以撤销的状态变更，撤回去只会让
    一批已经存在的子 agent 变成非法。`set_agent_limits` 不追溯。
11. **工具子集排序去重后落 `AgentValue::Json` 的字符串数组**（红线 11）：它会被渲染
    进子 agent 的 prompt，顺序一漂前缀缓存就全价。没有新增 `AgentValue` 变体——
    026 把值 schema 定死了，事后加变体等于一次 schema 迁移。

### 验收对照

| 验收 | 落点 |
|---|---|
| `root/a1` vs `root/a10` 边界；自身不是自己的祖先 | `ids/agent.rs` 的 `a_prefix_that_is_not_a_path_boundary_is_not_an_ancestor` / `the_boundary_rule_holds_deeper_down_too` / `nobody_is_their_own_ancestor`（外加 `is_ancestor_of` 的 doctest） |
| 子读父 Messages ✓ / 父读子 Status ✓ / 兄弟与方向错误显式拒绝 | `tests/session_subagent_read_boundary.rs` 十个用例 |
| `visibility()` 穷举且两方向集合不相交 | `graph/visibility.rs::the_three_visibilities_partition_every_slot` +（公开面）`no_slot_is_readable_in_both_directions` |
| spawn 记账：多一条 entry、changes 含子的初始槽位 | `tests/session_subagent_spawn.rs::a_spawn_lands_exactly_one_entry_carrying_the_childs_initial_slot` |
| undo 一轮连带子树 | `tests/session_subagent_undo.rs::one_undo_turn_takes_the_whole_subtree_with_it` + `redo_brings_the_whole_subtree_back` |
| despawn → undo → 子树值完整重建 | 同上 `undoing_a_despawn_rebuilds_the_subtree_with_its_live_values` / `a_rebuilt_child_keeps_working` |
| 019 三约束逐条 | 记 prev：`session_subagent_despawn.rs::the_teardown_entry_carries_every_live_value_as_prev`；自叶向根：`the_whole_subtree_comes_apart_leaf_first`；状态驱动拒绝：`src/command/despawn.rs` 的 `an_outside_reader_refuses_the_whole_despawn`（造一条外部读边）与 `a_slot_the_childs_derived_still_reads_cannot_be_evicted_first`（顺序反了引擎就拒） |
| 深度 4 / 子数 9 被拒，不 panic | `tests/session_subagent_spawn.rs` 的两个用例，返回 `SpawnRefused` 值 |
| 单 agent 路径零回归 | 23 个 `session_*` 测试二进制全绿（下面命令输出） |

### 收工命令输出

```
$ cargo test --workspace
cargo test exit=0
passed: 768  failed: 0  ignored: 0

$ cargo clippy --workspace --all-targets -- -D warnings
clippy exit=0        # warning/error 行数 0

$ bash scripts/check-invariants.sh --all
红线检查通过
规则与理由：docs/INVARIANTS.md
invariants exit=0

$ find crates/agent-core/src -name '*.rs' | xargs wc -l | awk '$1>300'
（空 —— agent-core 全部 ≤300，最大的是 despawn.rs 289）
```

**关于 768 这个数**：开工前实测基线是 691（与 CLAUDE.md 一致）。本 issue 新增
`#[test]` 55 个（`ids/agent.rs` 8 + `graph/visibility.rs` 3 + `spawn.rs` 4 +
`despawn.rs` 2 + 六个 `session_subagent_*.rs` 共 36 + `event.rs` 的 agent 提取器
1）加 1 个 doctest。剩下的差额来自 `crates/agent-server/`——**同一时段并行落地**
的另一条 M3 链（目录时间戳 09:25，在基线测量之后才出现），本次收工期间它自己还在
涨（三次全量测分别是 765 / 768 / 768）。跟 019 当年的处境一样，总数是移动靶；
可比的硬事实是 **failed 恒为 0，且 23 个 `session_*` 测试二进制全绿**。

### 推给 029 的

1. **`Notice` 没有 agent 归属**。它的文档注释写着「M3 多 agent 并行输出时要能分辨
   谁说的，那时加（issue 006 定了子 agent 形态之后）」——006 已经定了，029 接线时
   多 agent 的输出会分不清谁说的。加字段要动一个已经跨 SSE 的公开枚举，属于 029。
2. **子 agent 没有 `begin_turn`**。`turn_id` 只在 root 铸（决策 5，本 issue 落定），
   子 agent 的轮状态出生于 `spawn_child`（槽位默认值 = 刚开一轮）。跨 root turn
   还活着、还要接新输入的子 agent 是 029 的形态：那时它需要一条「重置本 agent 轮
   状态但不铸新 turn_id」的命令，或者干脆每轮 spawn 新的。
3. **取消仍然是会话级的**。`Effect::CancelInFlight` 没有 agent 字段、epoch 是会话
   世代——`Cancel { agent: child }` 会作废整个会话在飞的东西，包括 root 的。
   这是 STATE-MODEL 既有语义，本 issue 只是让它第一次真的能被触发到。按 agent
   取消要不要做，029 用真实场景判。
4. **取料读口还是 root 专属**（`messages()`/`status()`/…）。029 要给子 agent 组装
   `Ingredients` 时需要 per-agent 的取料口。**别把它做成第三个跨 agent 读 API**：
   宿主替某个 agent 取自己的料不是跨 agent 读（不产生图上的边），但形状上很像，
   写错就是红线 10 的洞。
5. **汇聚 derived 一律 family 现查、禁止闭包里焊 `AtomId`**（红线 4 孪生条款）。
   029 的「等所有子完成」要读的两样东西已经就位：活名单是各子的 `ToolsAllowed`
   （非 `Null`），完成与否是各子的 `Status`，两者都在 `Downward`。
   复杂度按 STATE-MODEL 的要求明确选一个：`Pending` 能短路就短路。

### 合并记录（主会话）

双侧零分歧：独测 32 测试一次全绿（含划分性质断言、读口无副作用实检、undo-spawn
裁决的行为面、墓碑三代不复用号）。StillRead 黑盒不可达如实记录不造假——029 的
汇聚读边落地后补。两处偏离原文（step 签名不动、读口双端点）均收。
workspace 768/0 → 加独测后 800/0。
