# 209 `Slot::Notes` + `srv:agent/notes`：模型自己的草稿纸

**里程碑** M20 · **依赖** [204](204-agent-mesh-decision.md)（拍板） · **模型** sonnet · **独测** ✅ · **状态** ✅ 完成（2026-08-18）

## 目标

决策 204 §三 的后半：用户要「改本 agent 状态」，**给它一个属于它自己的槽位，
而不是给现有槽位开写口**。

现有槽位一格一格数过去没有一格是模型的（204 §三 那张表：`MaxTurns` 是部署方的、
`ToolsAllowed` 是父给的、`SendPlan`/`Summaries` 是 adapter 的、`Status` 是父要读的）。
**给它们开写口 = 让被约束者改自己的约束。**

新槽位不碰任何现有不变量，而且**白拿全套机制**：`/undo` 连带撤销、崩溃恢复自动带回、
审计看得到每一次改。这是本仓架构直接掉出来的，不是新造的机制——**210 的真机
dogfood 要把这一点演出来。**

## 做什么

### 1. `Slot::Notes`

```rust
/// 这个 agent 自己记的东西。**只有它自己读得到、写得到。**
Notes,
```

- `visibility()` 里显式站 **`Private`**（`visibility.rs:59` 无通配穷举，不站队编译不过）。
  理由写进那条分支的注释：模块文档的规矩是「开放一个方向要有理由，封闭不需要」；
  子 agent 要父的上下文走 `Messages` 那条上读边，不该经由草稿纸。
- 值形状 **`BTreeMap<Arc<str>, Arc<str>>`**——**红线 11**：它会进 prompt，必须是有序容器。
  写成 `HashMap` 功能完全正常，只是每一轮全价（DeepSeek 上 120 倍）。
- `slot_default.rs` 给空表。红线 3：可序列化。

### 2. 两个命令 + 两个工具

| 工具 | 语义 |
|---|---|
| `srv:agent/notes` | 读：回本 agent 的全部条目（按 key 序）。无入参 |
| `srv:agent/notes/set` | 写：`{ key, value }`。`value` 为 `null` / 空 → 删这条 |

- 写走命令层（红线 2）。可逆性 `Aftermath::Nothing` → `Undoability::StateOnly`
  ——**M19 那套里最干净的一格**，undo 直接回滚状态，不需要任何钩子。
- 读是纯读，不落 entry、不调 `persist::sync`（照 `status_tool::intercept` 的既有理由）。
- 单条 `value` 有大小上限，照 004 的工具结果上限同款处理（超了截断并如实说）；
  条目总数也要有上限——**它每一轮都进 prompt**，无上限等于给模型一把慢慢烧钱的枪。

### 3. 什么时候进 prompt

**只在模型自己调 `srv:agent/notes` 时**，作为 tool_result 进。

**不做「自动注入进 system 前缀」**——那会让每一次写 notes 都动 system 前缀，
把前缀缓存打掉，是红线 11 要防的那类代价的另一半。草稿纸是模型自己要看时去查的东西，
不是背景板。

## 验收

- 写一条 → 读回来一模一样；写同 key 第二次 → 覆盖；写 `null` → 删掉。
- **`/undo` 掉写 notes 的那一轮 → 那条真的没了**（读一次 atom 断言，不是看日志说撤了）；
  **不产生屏障**（`StateOnly`）。
- **崩溃恢复**：写几条 → `kill -9` → 恢复 → 条目逐字节回来。
  （照 M18 的规矩：这条要真 `kill -9`，不是优雅退出。）
- **红线 11**：三条 key 乱序写入 → 读出来是 key 升序，且连读两次逐字节相同。
- **`Private` 守住**：兄弟 / 父 用 `peek_agent` 读 `Notes` → `NotVisible`
  （205 落了 `peek_agent` 之后这条才测得了；205 没落就先测
  `read_ancestor`/`read_descendant` 拒它）。
- 子 agent 的 notes 与父的**互不影响**（同一个 store、靠 family 的 `AgentId` 区分）。
- 超上限：单条超长 → 截断并如实说；条目数撞顶 → 显式拒，不静默丢。
- `the_three_visibilities_partition_every_slot` 全绿：`Notes` 进 `Private` 组，
  两个方向的集合一个都没变。
- `cargo test --workspace` 全绿 + `check-invariants --all` 过 + `build-wasm.sh` 绿。

## 注意

- **别做成 `Shared`**（横读全开之后它就不只是「子继承父」，是**所有人都读得到**）。
  听起来方便，但它会让「一个 agent 改一个 key」变成影响别人下一轮 prompt 的事，
  而模型完全看不到这条因果。要共享上下文有 `Messages`，要传话有 `send`。
- **别做跨 agent 的 notes**（A 写 B 的）。要给 B 传话有 `srv:agent/send`（206），
  它落在 B 的对话历史里、B 看得见是谁说的；偷偷改 B 的草稿纸则是 B 看不见来源的
  状态变化。
- **别自动注入进 prompt**，理由见 §3。
- 命名上别叫 `memory`——本仓已经有 skills / prefix / summaries 三样跟「记忆」沾边的
  东西，再来一个同名概念只会让文档更难读。`notes` 说的就是它是什么：一张草稿纸。

## 实做记录（2026-08-18）

### 值形状：`BTreeMap` 是承重的，不是图省事

`crate::value::notes::Notes` = `BTreeMap<Arc<str>, Arc<str>>`，落盘是按 key 升序的
`[[key, value], …]`。`to_value` **没有一句 `sort`**——`BTreeMap` 的迭代本来就是
key 升序，那正是选它的理由。换成 `HashMap` 的那天这个函数一个字都不用改，而落盘
字节会开始每轮不同（红线 11：功能完全正常，只是每一轮全价）。所以类型别名本身
就是那条不变量的载体。

跟隔壁 `inbox` / `summaries` 刻意不同：那两个是**流水账**（顺序 = 事情发生的先后，
同一个 key 可以出现两次），这个是**一张表**（同一个 key 写第二次是覆盖）。
用 `Vec` 就得在每个写入点自己做「找到就替换、找不到就追加」，写漏一处的症状是
同一个 key 在表里出现两次、读回来看哪一份取决于遍历方向——不报错。

### 三道闸，三种处理，不是一种

| 撞什么 | 谁处理 | 怎么处理 | 为什么 |
|---|---|---|---|
| 条目数 | core | 显式拒 `TooManyNotes` | 静默丢的症状是模型以为记下了、下一轮查不到，然后再记一遍 |
| 正文长度 | **工具层截断 + 如实说**；core 那一层仍然拒 | `truncated_content_bytes`（UTF-8 安全） | 模型基于一段残缺的笔记下结论，比拿到一句拒绝糟得多 |
| key 长度 | 两层都拒，**不截断** | — | 截短的 key 是**另一个名字**：模型下一轮拿原名查不到，而它记的时候明明成功了 |

按字节硬切会把一个中文字劈成半个，那个值会进状态、进 prompt，直到序列化时才炸
——所以截断走 004 那个既有的 `truncated_content_bytes`，不自己写一遍。

**撞顶之后覆盖一条已有的 key 仍然放行**：闸只拦新增。不然模型会陷在
「满了 → 想改一条腾地方 → 也被拒」里，而它没有别的出路。拒绝文案里把这条
写出来了（「覆盖已有的 key 不占新格子」）。

### 一档开两个工具

`with_notes()` 同时声明 `srv:agent/notes` 与 `srv:agent/notes/set`，跟别的 `with_*`
「一档一个工具」刻意不同（照 `with_skills` 的先例）。判据是**「一档是一件事」而不是
「一档是一个工具」**：只读不写的草稿纸永远是空的，只写不读的记了没人看得见——
两个半边各自都没有意义，拆成两档只会造出两种没用的组合。

### 不自动注入进 prompt

草稿纸只在模型自己调 `srv:agent/notes` 时作为 tool_result 进。自动注入进 system
前缀会让**每一次写 notes 都动 system 前缀**、把前缀缓存整段打掉——红线 11 要防的
那类代价的另一半。写成功的回执里因此明说了「它不会自动出现在你以后的对话里」：
不说清，模型会把「记下了」当成「下一轮我会看见」。

### issue 里两处过期的引用（写这份 issue 时 205 还没落地）

- `the_three_visibilities_partition_every_slot` → 205 之后 `Visibility` 只剩两态，
  测试叫 `the_two_visibilities_partition_every_slot`。
- `peek_agent` → 跨 agent 读的正门是 `read_agent`（205 落的名字）。

派独立测试 agent 时把这两处直接在 prompt 里更正了，没让它去撞。

### 红线 9

`graph/slot.rs` 加完 `Notes` 的文档 317 行 → 拆出 `slot_all.rs`。拆的位置正是
`graph/mod.rs` 那句用「以及」连起来的描述（「一个槽位怎么称呼，**以及**有哪些槽位」）
——**说不清一句不含「和/以及」的话，就是两件事**。

`Slot::ALL` 22 → 23，十一处钉住条数的测试连注释一起改（照 154 加 `HostPrefix` 的
做法，不只改数字）。
