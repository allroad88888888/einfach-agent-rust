# 051 `srv:agent/status` 工具——模型侧非阻塞观测

**里程碑** M8 · **依赖** 046（`agent_tree()`） · **模型** sonnet · **独测** ✅（碰红线 11）

模型侧编排的**观测半边**，且独立可先发：一个纯读工具，让模型（一个父 agent）拿到自己子树
里每个后代此刻在干啥。不碰状态模型、不碰 pump，自己就落「模型看得到子 agent」。接缝见
[ORCHESTRATION.md](../ORCHESTRATION.md) §三/五。

## 范围

1. **工具声明**（`agent-runtime/src/tool_table.rs` + 新 `status_tool.rs`）：`srv:agent/status`，
   Server 位置，可逆性 `Pure`（纯读、无副作用、无屏障）。参数：可选 `id`（省略=调用者自身
   子树；给了必须是调用者的后代，否则 `is_error`）。照 `spawn_tool.rs` 的组织方式（声明住
   agent-runtime，因为要读 `Session`）。
2. **dispatch 截获**（`dispatch.rs` 的 `Effect::ExecuteTool` 内，spawn/skill 同款位置）：命中
   `srv:agent/status` → 调 `session.agent_tree()`，用 `AgentId::is_descendant_of` 把 `nodes`
   收窄到「调用者的后代」（红线 10 下读方向）→ 序列化成 tool_result 正文，**当场回写**
   （无 Pending、无在飞）。
3. **tool_result 正文格式**：给模型读的紧凑文本/JSON，每个后代一行：`id / depth / activity
   (Idle|Thinking|Working{tools}|Done|Failed{reason}) / task`。**只暴露 activity + task，不暴露
   子的消息正文**（正文是 collect 的事、走另一条路）。

## 验收（可判定）

- 父 spawn 两个子（前台或后台皆可构造），子在跑时父调 `status` → 回来的 tool_result 列出
  两个后代及其 activity，且**只含调用者的后代**（兄弟/祖先的其它分支不出现——红线 10）。
- `status(id=<非后代>)` → `is_error` 的 tool_result（拒绝横读/上读），loop 继续不 panic。
- **红线 11（进 prompt 逐字节确定）**：`status` 结果进下一轮 prompt。`nodes` 必须按 `AgentId`
  路径**稳定排序**，序列化禁 `HashMap`/`HashSet`。独测断言：同一棵树两次序列化字节相同。
- `agent_tree()` 是既有派生读（046），本 issue **不新增 atom / 不新增 primitive**。

## 注意

- **红线 11 是本 issue 唯一的静默失败点**（结果进 prompt，非确定序列化 → 功能正常但每轮全价
  缓存失效）。`agent_tree()` 已 `derive(TS)`，检查它 `nodes` 的产出顺序是否确定——`live_agents`
  的迭代序若不稳，这里补一次 `sort_by(AgentId)`。**派独测**断言字节确定。
- **红线 10**：只下读。`id` 参数必须过 `is_descendant_of(caller)` 校验，横读/上读拒。
- **红线 12**：按工具名截在宿主 dispatch，core 不碰。
- 本 issue 不依赖 052/053——可**先于**异步核心落地、独立验收（阻塞 spawn 场景下父在子跑
  时也能调 status，只要构造出「子在飞、父在下一轮」的时刻；最稳的构造是父发多个并行前台
  spawn 时另一个 status 调用……若难构造，用后台 spawn 场景等 052，但工具本身实现与 052 无关）。
- 收工验证前台跑完（WORKFLOW §四 -1），别后台自旋。

## 实做记录（完成 · 2026-08-04）

一个纯读工具落地：声明 + 截获 + 收窄 + 渲染。**没有新增 atom / primitive / 事件 /
effect**——它整个就是 046 那个 `Session::agent_tree()` 的一次读，加一层住在宿主的收窄。

### 建了什么

| 文件 | 行 | 干什么 |
|---|---|---|
| `agent-runtime/src/status_tool.rs` | 288 | 新：`STATUS_TOOL` 全名、`status_spec()` 声明、`parse`（可选 `id`）、`observe`（收窄 + 校验 + 渲染）、`intercept`（dispatch 截获体）。收窄与排序都在这里 |
| `agent-runtime/src/status_tool_tests.rs` | 267 | 新：收窄/拒绝/解析/**红线 11 字节确定**/渲染字面形状的单测（`#[path]` 子模块，红线 9 同 043 的处置） |
| `agent-runtime/src/tool_table.rs` | 262（+24） | 改：`with_status()` 建造器；`reversibility_of` 加 `STATUS_TOOL => Pure` |
| `agent-runtime/src/tool_table_tests.rs` | 182（+30） | 改：`with_status` 追加位置 + `Pure` + 两个开关互不牵连 |
| `agent-runtime/src/dispatch.rs` | 236（+10） | 改：`Effect::ExecuteTool` 内第三处截获（spawn → **status** → skill → MCP → executor） |
| `agent-runtime/src/lib.rs` | 80（+3） | 改：`mod status_tool`、`pub use {STATUS_TOOL, status_spec}`、effect 表那行补一句 |
| `agent-cli/src/main.rs` | 229（+4） | 改：工具表 `.with_spawn(..).with_status().with_skills(..).with_mcp(..)` |
| `agent-server/src/registry/spec.rs` | 106（+3） | 改：`ToolTableSpec::Full` 这一档带上 status（这一档的意思就是「开子 agent」） |
| `agent-runtime/tests/status_indep_lists_descendants.rs` | 110 | 新：验收第一条——父同批发两个 spawn + 一个 status，再在下一跳调一次 status |
| `agent-runtime/tests/status_indep_only_descendants.rs` | 120 | 新：**红线 10**——中间层 agent 调 status，兄弟（正在飞）和祖先都不出现 |
| `agent-runtime/tests/status_indep_rejects_non_descendant_id.rs` | 98 | 新：验收第二条——上读/横读各一次，双双 `is_error`，loop 照常收工 |
| `agent-runtime/tests/status_indep_support/mod.rs` | 47 | 新：`#[path]` 复用 029 的假服务器/装配夹具 + 三个读正文的断言助手 |

### tool_result 正文的形状

一个后代一行，四段定长字段，标题带计数：

```
你的子 agent（2 个，只列你自己的后代）：
root/a1 depth=1 Working(srv:fs/read) task=分析 A
root/a2 depth=1 Done task=分析 B
```

`activity` 的五种写法跟 ORCHESTRATION.md §三那张表逐字对得上：`Idle` / `Thinking` /
`Working(工具名,工具名)`（在飞工具名一时推不出来时退成裸 `Working`，不写一对空括号）/
`Done` / `Done(truncated)` / `Failed(原因)`。`task` 压平换行、按**字符**截到 100 加 `…`
（按字节切会切碎中文）。没有后代时回一句话（"你现在没有子 agent……"），不回空正文让模型猜。

**只有 activity + task，没有子 agent 的正文**——`AgentNode` 压根没有装正文的字段，
而单测 `the_body_carries_activity_and_task_only` 断言每行恰好四段，守的是「将来别人给它
加一个」。e2e 那条更直接：第二跳时子的回答就躺在同一条历史的隔壁块里（spawn 的
tool_result），status 正文里仍然不许有它。

### 接口决策一：收窄住宿主，不住 core

`agent_tree()` 返回**整棵**活树，`status_tool::observe` 把它收窄成「调用者的严格后代」。
两个理由，缺一条都还不够：

1. **职责**：「谁在调、他能看到哪些」只有宿主知道（`Effect::ExecuteTool` 的 `agent`
   字段在 dispatch 手上）。给 core 的纯读加一个「按调用者过滤」的参数，等于把宿主的
   视角概念推进一个本来只回答「世界现在长什么样」的函数。
2. **落地**：`agent-core/src/observe.rs` 已经 291 行，红线 9 只剩 9 行余量——把收窄塞
   进去就得先拆它，而拆的理由是假的（那个文件本来就只干一件事）。

**红线 10 因此是由构造保证、不是由检查保证**：`observe` 先算出 `mine =
descendants(tree, caller)` 这**一个**集合，`id` 那条路只在它里面再 `filter` 一次。
结构上无从放大视野——不存在「某条校验分支漏了就横读」的可能，因为根本没有第二条能
产出节点的路径。

`id` 必须是**严格**后代，`id=<自己>` 也拒：规则一条没有例外比多一个「除非是你自己」
的旁支好记，而拒绝文本直接告诉模型「省掉 id 就是你想要的那件事」。拒绝分两种（"不是你的
后代" vs "不在你的活子树上"），因为模型的下一步不一样；两种都把「你能看的是哪些」列出来
（`spawn_tool::check_subset` 同款写法，003 的哲学）。

### 接口决策二：截获**不**调 `persist::sync`

spawn / skill 那两处截获都在成功后调了 `persist::sync`——因为它们各自落了一条 `Entry`
（`spawn_child` / `activate_skill` 是命令）。status 一条命令都没发、一个 primitive 都没写、
零 entry，所以没有任何东西需要同步。它也因此**不进 `mark_irreversible`**：`Pure` 的定义
就是「无副作用、无需补偿」，日志上不留屏障位，`/undo` 路过它时不停下来问（问了也没有
东西可撤）。当场回写、无 Pending、无在飞凭据——这是它跟 MCP 第四路的分水岭。

### 红线 11：字节确定性怎么保证的

这段正文是 tool_result，从此**每一轮**都躺在调用者的历史里进 prompt。三道：

1. **自己排序**。`descendants()` 收完之后 `sort_by(|a, b| a.id.cmp(&b.id))`。
   `live_agents()` 今天确实是有序的（`known_agents()` 是 `BTreeSet` + 一次 `sort()`），
   但那是**被调方**的实现承诺——它哪天改了，坏掉的是这段进 prompt 的字节，而它自己的
   测试还是绿的。确定性要在用得着它的地方自己保证，不借。
2. **全程有序容器**。`Vec` + `Vec<&AgentNode>`，`HashMap`/`HashSet` 一个没有
   （`check-invariants.sh` 的红线 11 grep 也在盯这一条）。
3. **渲染是纯函数**。压平换行、按字符截断、activity 的字面写法，都不读时钟/随机/环境。

独测两条断言把它变成会红的东西：`the_same_tree_renders_to_the_same_bytes_twice`
（同一棵树两次 → `as_bytes()` 相等）和 `a_shuffled_node_order_renders_to_the_very_same_bytes`
（把 `nodes` 反转、再换两组位置 → 输出字节仍然逐字相同）。第二条是真正管用的那条：
把 `sort_by` 那一行删掉，它立刻红。

### 一个如实记下来的窗口：同批 spawn 出来的子还没有 task

阻塞 spawn 下，父唯一能在「子还在跑」时调 status 的时刻，是**同一条 assistant 消息里
spawn 和 status 并列**（051 §注意点名的构造）。而那一刻子的 `task` 是 `(无)`：任务文本是
子 agent 的第一条 user 消息，由 spawn 截获产出、排在泵的待办队列里，要等下一次 `step`
才写进去（`dispatch.rs` §「任务文本 = 子 agent 的第一条 user 消息」）。同批派发的 status
撞的正是这个窗口。

**没有绕**：绕它要么让 `spawn_child` 顺手写一条 user 消息（改的是 029 的命令语义），
要么在 dispatch 里插一次 `session.step`（跳过泵的持久化与树通报）——两条都超出一个纯读
工具的范围。`task=(无)` 是如实报告，不拿 id 顶替一个假任务。窗口只有一批那么宽，父的
下一跳就有了（e2e 的第二次 status 断言了这一点）。052 的后台 spawn 落地后这个窗口自然
消失：那时 status 天然发生在建子之后的另一批里。

### 坑

- `root/a1` 是 `root/a1/a1` 的**子串**——`body.contains("root/a1")` 这类断言在 agent 树上
  是假绿灯，红线 10 破了测试还能是绿的。所有 id 断言一律走 `listed_ids()` 逐行取第一个
  字段比集合（`AgentId::is_ancestor_of` 当年踩的同一个坑，换了个地方长出来）。
- 「兄弟没出现」和「兄弟还没被建出来」在结果上长得一模一样。所以 `only_descendants`
  那条让兄弟那一路慢 600ms，并用假服务器记的真实时间窗断言
  `left_hop2.start < sibling.end`——status 读树的那一刻兄弟确实在飞。
- `status_tool.rs` 加上单测必然顶破 300 → 单测挪进 `#[path]` 子文件（源文件只留实现），
  和 043 处置 `ctx.rs`/`tool_table.rs` 同款，不是硬塞。

### 收工验证（前台跑完，真实输出）

三道门禁一次过，无返工。

```
### TEST: cargo test -p agent-runtime -p agent-core ###
（每个测试二进制一行 test result，全部 ok；下面是逐条求和）
>>> 合计: 502 passed, 0 failed
   Doc-tests agent_core     ... ok. 6 passed; 0 failed
   Doc-tests agent_runtime  ... ok. 0 passed; 0 failed
其中本 issue 新增的四组：
   status_tool::tests                              19 passed（含两条红线 11 字节断言）
   tests/status_indep_lists_descendants.rs          ok. 4 passed（1 用例 + 3 条夹具自检）
   tests/status_indep_only_descendants.rs           ok. 4 passed
   tests/status_indep_rejects_non_descendant_id.rs  ok. 4 passed
   tool_table::tests::with_status_appends_the_status_tool_and_it_is_pure ... ok
   tool_table::tests::a_table_without_status_does_not_declare_it        ... ok

### CLIPPY: cargo clippy -p agent-runtime --all-targets -- -D warnings ###
    Checking agent-runtime v0.1.0 (/Volumes/work/self/einfach-agent-rust/crates/agent-runtime)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 38.73s
（0 warning，一次过——043 记的那条教训：clippy 那道门不确认不算收工）

### INVARIANTS: bash scripts/check-invariants.sh --all ###
红线检查通过
规则与理由：docs/INVARIANTS.md
（`git ls-files '*.rs'` = 494 个文件，确认扫到了；单独喂 status_tool.rs /
 tool_table.rs / dispatch.rs 三个改动文件也通过。288/262/236 行，红线 9 全在 300 以内）

### 顺带：被接线的两个宿主没被弄坏 ###
cargo test -p agent-server -p agent-cli
>>> 合计: 155 passed, 0 failed
```

**过程如实记**：`cargo test -p agent-runtime -p agent-core` 首跑（含全部测试二进制的
冷编译）超过了工具 600s 的上限被挪去后台。**没有转成后台自旋**——用 `caffeinate -w <pid>`
在前台阻塞等它退出（不轮询、不占 CPU），拿到真实退出码 0，再重跑一遍收全部逐条 test
result 求和。收工时 `ps` 确认本 issue 没有留下任何 cargo/rustc 孤儿进程（当时在跑的两个
属于另一个仓 `/Volumes/work/self/excel` 的并行会话，不碰）。
