# 149 扩展真机 dogfood：一个真扩展包走全程 ← M16 前半终点

**里程碑** M16 · **依赖** [147](147-migrate-intercepts.md) + [148](148-extension-pack-seam.md) · **模型** **opus** · **独测** 本条即验收 · **状态** 完成（见文末）

## 目标

用 146–148 的机制写一个**真扩展包**并真机走全程（真 provider），验证
「第三方 Rust 扩展」这条路从写包到装配到模型使用到 undo 的完整闭环。
132/143 的先例：dogfood 专抓「各条 issue 各自绿、合起来漏」的缝。

## 扩展包内容（示例但要真有用）

`ext:stats` 包，两件东西：

1. **`ext:stats/report`**（截获式，Pure）：读 `agent_tree()` + 各 agent 的
   `messages_of` + entry label 序列，给模型吐一份「本会话至今：几轮、几次
   工具调用、几个子 agent、undo 过几次」的文本汇总。
2. **TurnEnd hook**：每完成轮把「轮号 + entry 数」追加进一个本地审计文件
   （宿主侧文件，不进状态）。

落点：`agent-cli` 内一个 feature 门或 `--ext-stats` 开关后的模块（实现者
选最小侵入的一种并记录理由）；**不新开 crate**——第一个扩展包先证明接缝，
包的独立发布形态等真有第三方再说。

## 验收（逐条可判定）

1. CLI 真机（DeepSeek）：问「这个会话到目前为止干了什么」→ 模型自主调
   `ext:stats/report` 并用返回内容回答。
2. **undo 的活演示**（这条是账本卖点的正面戏）：spawn 一个子 agent 干点活 →
   调 report 记下数字 → `/undo` 撤掉那轮 → 再调 report → **数字跟着回退**
   （树少一个节点、entry 数回落）——扩展读到的世界与账本严格一致。
3. TurnEnd 审计文件每完成轮恰好多一行；取消轮不多。
4. 不装包的会话：specs/prompt 逐字节与 M16 之前相同。
5. 十轮 `cached/prompt ≥ 0.9` 照旧（扩展工具结果走消息尾，不破前缀）。
6. `kill -9` 恢复后再调 report，数字与崩溃前一致（读的是恢复出的状态）。

## 回填

- 逐条兑现记录；发现的交界 bug 就地修并记录（132 先例）。
- `docs/EXTENSIONS.md` 补「写你的第一个扩展包」一节（以 ext:stats 为教材）。
- ROADMAP §二 记 M16 前半完成；150 的决策拿本条的手感当输入。

## 注意

- 花真钱：单发，不并发跑两个实验（WORKFLOW §四 -2）。
- report 的输出要过工具结果上限（决策 19，32 KiB）——长会话下自己截断，
  别指望 core 兜底截得好看。

## 实做记录：第一个真扩展包走完全程（2026-08-12，CLI + 真 DeepSeek key）

**六条全过。** provider = deepseek/deepseek-v4-flash，模型侧一共 16 跳真实请求。

落点（都在 `agent-cli` 里，不新开 crate）：

| 文件 | 行 | 管什么 |
|---|---|---|
| `crates/agent-cli/src/ext_stats.rs` | 268 | 包本身：两条条目、`Ledger`、`--ext-stats` 开关、两阶段装配 |
| `crates/agent-cli/src/ext_stats_report.rs` | 251 | 正文渲染（纯函数：`&Session` + 调用者 → 字节 + `Counts`） |
| `crates/agent-cli/src/vision.rs` | 36 | 从 `main.rs` 拆出（装配链加长后那个文件顶破 300 行） |
| `crates/agent-cli/src/main.rs` | 295 | 装配加 6 行：`install(...)` + `pending.install(&mut ctx)` |
| 两份 `*_tests.rs` | 232 + 196 | 18 个单测（含验收 4 的表级断言、undo 回落的单测孪生） |

**开关不是 feature 门**（issue 原文给了两个选项）：验收 4 是「同一个二进制跑两次、
一次带开关一次不带、请求体逐字节相同」——feature 门下这两次是两个二进制，反而要靠
比对构建产物说话。

### 1 模型自主调 report ✅

第 2 轮，用户只问「这个会话到目前为止干了什么？」：

```
[2m用户问这个会话到目前为止干了什么。我应该用 ext_3Astats_2Freport 来看账本汇总。[0m
[tool] ext:stats/report {} (location=Server reversibility=Pure)
      -> ext:stats/report 完成，402 字节
[2m账本显示：2 轮、6 条 entry、工具调用 0 次。[0m
到目前为止其实没干什么实质性的活：共 2 轮对话，6 条账本记录，工具调用 0 次……
```

**没改过 description**（issue §注意 预留的那条退路没用上）：两次真机跑、五次询问，
五次都是模型自己决定调它，答案里的数字逐个来自返回正文。

顺带一条 adapter 事实：DeepSeek 侧工具名被编码成 `ext_3Astats_2Freport`
（`:`→`_3A`、`/`→`_2F`，[050](050-tool-name-encoding.md) 那套编码），回来时原样解回
`ext:stats/report` 命中截获表——`ext:` 这个新前缀过 adapter 往返没有额外摩擦。

### 2 undo 活演示：数字跟着账本回退 ✅（本条是主戏，两次正文逐字抄录）

第 4 轮（spawn 过一个子 agent 之后）模型拿到的 tool_result **原文**：

```
本会话至今：4 轮、19 条 entry、2 个 agent、工具调用 2 次。
账本：turn_id=4，entry 生效 19 / 物理 19（可 redo 0），epoch=0，屏障 0 处。
entry 分布：begin_turn×3、prefix_init×1、provider_done×7、spawn_child×1、tool_result×2、user_input×5
你自己（root）：消息 12 条，Working(ext:stats/report)。
你的子 agent（1 个，只列你自己的后代）：
root/a1 深度1 Done task=只计算 17*23，只回答数字，不要调用任何工具。
```

两次 `/undo`（`[已撤销] 第 4 轮，5 条` / `[已撤销] 第 3 轮，8 条`）之后，第 5 轮同一个
工具的 tool_result **原文**：

```
本会话至今：3 轮、11 条 entry、1 个 agent、工具调用 1 次。
账本：turn_id=5，entry 生效 11 / 物理 11（可 redo 0），epoch=2，屏障 0 处。
entry 分布：begin_turn×2、prefix_init×1、provider_done×4、tool_result×1、user_input×3
你自己（root）：消息 8 条，Working(ext:stats/report)。
你现在没有子 agent：还没 spawn 过，或者它们那一轮已经被撤销了。
```

模型据此答：「agent 数：1（只有我自己，没有子 agent）／entry 数：11／工具调用次数：1」。

逐项回退：agent 2 → 1（`root/a1` 从树上消失）、生效 entry 19 → 11、工具调用 2 → 1、
`spawn_child×1` 整条从分布里消失、epoch 0 → 2（两次 undo 各 bump 一代）。
**扩展这一侧没有一行代码认识「撤销」**——它只是又读了一次 `agent_tree()` 与
`history()` 的生效段。这正是账本卖点的正面戏：第三方扩展读到的世界与 undo 严格一致，
不需要扩展作者做任何事。

（`/undo` 没被屏障挡：spawn 是 `Reversible`、report 是 `Pure`，路上没有 `barrier: true`
的 entry——`屏障 0 处` 这行数字自己也报出来了。）

### 3 审计文件：每完成轮恰一行，取消轮不加行 ✅

**九个完成轮的会话**（`b.jsonl.audit.log`，`wc -l` = 9）：

```
turn=1 entries=- turns=- agents=- tools=- seen_at=-
turn=2 entries=6/6 turns=2 agents=1 tools=0 seen_at=turn2
turn=3 entries=6/6 turns=2 agents=1 tools=0 seen_at=turn2
turn=4 entries=19/19 turns=4 agents=2 tools=2 seen_at=turn4
turn=5 entries=11/11 turns=3 agents=1 tools=1 seen_at=turn5
turn=6 entries=11/11 turns=3 agents=1 tools=1 seen_at=turn5
turn=7 entries=11/11 turns=3 agents=1 tools=1 seen_at=turn5
turn=8 entries=11/11 turns=3 agents=1 tools=1 seen_at=turn5
turn=9 entries=25/25 turns=7 agents=1 tools=2 seen_at=turn9
```

**取消轮**（另起一个会话，三条输入、中间那条 `kill -INT` 打断）：

```
（第 1 轮）打招呼             → [本轮完成]        → 审计第 1 行
（第 2 轮）写 800 字散文 …… [本轮失败: Cancelled]
                                [已撤销] 取消的第 2 轮留下的 3 条痕迹已经擦除，没有计入历史
（第 3 轮）短问答             → [本轮完成]        → 审计第 2 行
```

三条输入、两个完成轮、**审计文件恰好两行**，第二行是 `turn=2`——序号数的是完成轮，
不是输入行。真 SIGINT（`kill -INT` 打给进程，走 `ctrlc` 那条 handler），不是测试模拟。

### 4 不开开关：请求体与 M16 前**逐字节相同** ✅（sha256 相等，不是「看起来一样」）

拿一个本地 recorder 当 provider 端点（`base_url` 指到 127.0.0.1，收到请求把 body 落盘
再回 500），同一句输入 `你好` 跑三次：

| 跑法 | 二进制 | body sha256 | 字节数 |
|---|---|---|---|
| M16 前基线 | 149 动手前编的 `agent-cli` | `79bd1d5c3b88e3ab45cbccaec90ef31cc71e296594921e4ce522247a4c8269eb` | 23957 |
| 不开开关 | 本次改动后的 `agent-cli` | `79bd1d5c…4c8269eb`（**相同**） | 23957 |
| 开开关 | 同一个二进制 `--ext-stats` | `637ac3ad6431063611d8d2fbfa1a0fceb42a695af0ec9fc7ad76d97be8616f8e` | 24765 |

开关打开那份：`tools` 从 21 条变 22 条，**前 21 条逐条相同**，第一处不同的字节在
offset 23955（共 23957）——也就是工具数组的收尾括号处，整段追加在**表尾**，前面所有
会话共有的字节一个都没动（红线 11）。`messages`/`model`/`stream`/`stream_options` 全等。

这条同时是「零成本」的证据：不开开关时 `with_extension` 一次都没调，没有空包、没有
往可逆性映射插键、没有必须 install 的半边。

### 5 十轮缓存命中：14 跳 96.1%–99.0%，零条低于 0.9 ✅

| 跳 | 轮 | prompt | cached | 命中率 | 备注 |
|---|---|---|---|---|---|
| 1 | 1 | 6876 | 6784 | 98.7% | |
| 2 | 2 | 6899 | 6784 | 98.3% | report 调用跳 |
| 3 | 2 | 7092 | 6912 | 97.5% | report 结果回灌 |
| 4 | 3 | 7176 | 7040 | 98.1% | spawn |
| 5 | 3 | 7457 | 7168 | 96.1% | |
| 6 | 4 | 7497 | 7424 | 99.0% | report 调用跳 |
| 7 | 4 | 7722 | 7552 | 97.8% | |
| 8 | 5 | 7161 | 7040 | 98.3% | **两次 undo 之后的第一跳** |
| 9 | 5 | 7360 | 7168 | 97.4% | report 调用跳 |
| 10 | 6 | 7422 | 7296 | 98.3% | |
| 11 | 7 | 7474 | 7296 | 97.6% | |
| 12 | 8 | 7523 | 7424 | 98.7% | |
| 13 | 9 | 7602 | 7424 | 97.7% | report 调用跳 |
| 14 | 9 | 7794 | 7552 | 96.9% | |

**均值 97.9%，最低 96.1%**，五个 report 调用轮全部在内。扩展工具的结果跟 skill 正文
一样从**消息尾**进来，不破前缀——决策 27/29 共用的那个赌注在扩展这条路上同样成立。
（子 agent `root/a1` 那一跳是 250/0：新 agent 的第一跳本来就是冷的，同 143 的先例。）

### 6 `kill -9` 恢复：读的是恢复出的状态，数字对得上 ✅

第 9 轮完成后 `kill -9`（进程停在 `> ` 等下一行输入，没有任何优雅退出），重启：

```
[会话已恢复] 接着第 9 轮继续
```

| | 崩溃前（第 9 轮中的 report） | 恢复后（第 10 轮的 report） | 差 |
|---|---|---|---|
| 轮 | 7 | 8 | +1（新那一轮） |
| entry | 25 | 30 | +5 |
| agent | 1 | 1 | **0** |
| 工具调用 | 2 | 3 | +1（第 9 轮自己那条 tool_result） |
| epoch | 2 | 3 | +1（`recover` 取日志最大值 +1，meta.rs 的既有语义） |

label 分布逐项对得上：`begin_turn 6→7`（新轮）、`prefix_init 1→1`（**开局工具不重跑**）、
`provider_done 9→11`（第 9 轮的收尾答复 + 第 10 轮的工具请求）、`tool_result 2→3`、
`user_input 7→8`。差的正好是崩溃后新做的那些事，**崩溃前的每一个数字都原样在**，
没有一条 entry 丢失、没有一条被重放两次。审计文件也接着数：新行是 `turn=10`，不是
`turn=1`（`Ledger` 起手式按既有审计文件的行数续号）。

### 交界发现 1（本条 dogfood 的主要产出，[150](150-derived-extension-decision.md) 的输入）：**`TurnEnd` 钩子看不见 `Session`**

`TimedRun` 的签名是 `Fn(&ToolTable, &Value) -> Result<Arc<str>, Arc<str>>`（133 的 v1
边界）——**没有 `Session`**。于是 issue 原文那句「每完成轮把轮号 + entry 数写进审计
文件」在今天的机制上做不到「钩子自己去读账本」：钩子只知道自己被调过几次。

处置（**不改 133 的签名**——「触发 hook 与 TurnEnd 的关系」正是 150 要拍的事，
dogfood 的职责是把手感交给它，不是抢着定形状）：包自己带一格宿主内存里的
`Ledger`，截获那半边（`report`，它有 `&mut Session`）每次跑完把数字记进去，钩子写行
时如实标注 `seen_at=turnN`——**没人观测过的轮写 `-`，不拿零顶替**。上面第 3 条那份
审计文件里 `turn=3`、`turn=6..8` 几行就是这个形状：数字停在上一次观测，行照出。

这条缝的形状很值得 150 拿着看：`report` 是 `Pure` 的纯读，钩子要的也是纯读，可两者
中间隔着一个「谁能拿到 `Session`」的类型墙，只能靠宿主内存里的一格传话。150 要决的
「谓词触发 / 扩展 derived」如果把 hook 的入参面打开成「一份稳定态的只读视图」，这一格
传话连同它的 `seen_at` 标注就都可以删掉。

### 交界发现 2（真机第一次跑就现形，当场改）：`seen_at` 差一位

第一版 `observe()` 记的是 `turns.load()`（**已完成**轮数），而钩子在轮末才 +1——于是
「第 2 轮里观测到的数字」被写成 `seen_at=turn1`。审计文件里差一位的行号比没有行号更
坑人（照着它去 journal 里找那一轮会找错）。改成「正在进行的那一轮 = 已完成 + 1」，
单测钉死（`an_audit_line_reports_which_turn_the_numbers_came_from`）。

**只有真机能抓到它**：单测里钩子是手动调的，`observe` 与 `append_turn_line` 的相对
时序由测试自己摆，摆的是我以为的那个顺序；真机上顺序由 136 的驱动决定。

### 顺带的两条纪律观察（写给下一个扩展作者，已进 EXTENSIONS.md）

1. **`messages_of(child)` 是够得着但不许走的门**：`Slot::Messages` 是 Upward-only
   （`cross_read.rs` 的可见性表），`read_descendant` 会拒；而扩展手里是整个
   `&mut Session`，直接 `messages_of(child)` 拿得到。这份报告因此只报**调用者自己**
   的消息条数，子 agent 只报 status 那一档（id/深度/活动/task），跟
   `status_tool::observe` 逐字同一条线。
2. **报告里的每个数字都必须是状态的函数**。第一版曾想把钩子的轮计数也放进正文——那是
   进程内存里的数，`kill -9` 之后它归零，而报告是要进 prompt 的。真放进去，验收 6 会
   在「恢复后数字不一致」上失败，而且失败得很像一个状态 bug。扩展作者的自检问题：
   **这个数字撤销之后会回退吗？崩溃恢复之后还在吗？** 两个都答「是」，它才有资格进
   tool_result。
