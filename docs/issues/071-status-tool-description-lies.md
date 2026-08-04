# 071 `srv:agent/status` 的工具说明书对后台子 agent 说假话

**里程碑** 待归类（工具描述 / M8 遗留） · **依赖** — · **模型** sonnet · **独测** —

文档一致性审计（`docs/DOC-AUDIT.md`）捞到的**模型可见的错误信息**。

## 现象

`crates/agent-runtime/src/status_tool.rs` 的工具描述里写着（大意）：

> ……子 agent 的正文会在**那次 spawn 调用的结果里**回到你这里。

这在 051 写下时是对的（那时只有阻塞 spawn，结果确实从 spawn 槽回来）。**052 引入
`background: true` 之后就成了假话**：后台 spawn 只回 `{"agent_id":"root/a1"}`，正文要靠
053 的 `srv:agent/collect` 去领。

## 为什么要紧

1. **这段字符串每一轮都进 prompt**（工具表在最前面），模型每次都读到它。
2. **模型会照它办事**：以为发完后台 spawn 就能在结果里拿到正文 → 不调 `collect` → 轮末
   被孤儿收尾拆掉，工作白做（052 的告警会报，但那是事后）。
3. **没有任何测试断言工具描述文本**——审计特别点了这一条：工具说明书是模型的唯一接口
   文档，却是全仓唯一没有测试守的字符串。

有 `collect_spec()` 的反向提醒兜底（它的描述里说了「用 collect 领后台子的结果」），
所以这是**可用性缺陷不是数据事故**——但它每轮都在花钱说一句错话。

## 范围

1. **改对 `status_tool.rs` 的描述**：区分两种 spawn——前台（阻塞）的结果从 spawn 槽回来；
   `background: true` 的**只回 `agent_id`，正文要用 `srv:agent/collect` 领**。
2. **顺手核对另外两个描述**（同一类风险）：`spawn_tool.rs` 的 `background` 参数说明、
   `collect_tool.rs` 的描述——它们与 052/053 的实际行为是否一致。**发现不一致就一并改**。
3. **加测试守住**（本 issue 的长期价值）：断言这三个工具的描述里**含有关键事实**
   （如 status 的描述必须提到 `collect`、spawn 的 `background` 说明必须说清「不会自己回来」）。
   **别断言整段文本逐字相等**——那样每次改文案都要改测试，没人会维护。断言关键子串即可。

## 验收（可判定）

- `status_tool` 的描述不再声称后台子的正文会从 spawn 结果回来；提到用 `collect` 领。
- 三个描述与 052/053 的实际行为一致（逐条核对后在记录里列出来）。
- 有测试断言关键子串；**故意把描述改回旧文案 → 测试红**（做一次突变验证，贴真实输出）。
- **红线 11**：描述进 prompt，改它会让所有既有会话的前缀失效一次——这是**一次性代价**，
  在记录里如实标注（对比：不改的话是每轮都在说错话）。
- 既有测试不回归（`cargo test -p agent-runtime`）。

## 注意

- **只改描述文本 + 加测试**，不要动 `status`/`spawn`/`collect` 的任何行为逻辑。
- **不要碰** `crates/agent-server/`（062 在改）、`crates/agent-mcp/`（070 在改）、
  `crates/agent-tools/`（并发会话 WIP）。
- 红线 9：`status_tool.rs` 现在 263 行，余量不多；只加描述文本和测试的话应该够，
  测试放 `#[path]` 子文件（该文件已有 `status_tool_tests.rs` 的先例）。
- 收工验证前台跑完（WORKFLOW §四 -1）；主 target 被占就用独立 `CARGO_TARGET_DIR`。

## 实做记录（完成 · 2026-08-04）

**只改了描述文本 + 加了三条测试。** `status`/`spawn`/`collect` 的行为逻辑一行没动，
没有新文件、没有新模块、没有新 `mod` 声明。

### 改了什么

| 文件 | 行 | 改了什么 |
|---|---|---|
| `agent-runtime/src/status_tool.rs` | 266（263→，+3） | 描述：那句假话改成分前台/后台两条路说 |
| `agent-runtime/src/spawn_tool.rs` | 298（299→，−1） | 描述 + `background` 参数说明：加上「不会自己回来 / 用 collect 领」 |
| `agent-runtime/src/collect_tool.rs` | 229 | **一字未改**（核对过，一致） |
| `agent-runtime/src/status_tool_tests.rs` | 286（267→，+19） | 原 spec 测试扩写成「正文从哪来」的三条子串断言 |
| `agent-runtime/src/spawn_tool_tests.rs` | 123（102→，+21） | 新：`background` 那段的四条子串断言 + 参数说明两条 |
| `agent-runtime/src/collect_tool_tests.rs` | 267（244→，+23） | 新：collect 描述的四条子串断言（原先一条都没有） |

全部 ≤300（红线 9）。`spawn_tool.rs` 原本就贴着 299，所以新文案是**按行数配平**写的
（合并掉一句、少一行），不是「反正只是 warning」。

### 三个描述的逐条核对（对着 052/053 的实做记录与代码）

**一、`status_tool.rs` —— 不一致，就是本 issue 的 bug。**

| 描述里的话 | 实际行为 | 判定 |
|---|---|---|
| 「不阻塞，当场返回」 | `intercept` 无 Pending、无在飞凭据，`observe` 算完就 `reply::ok` | ✅ |
| 「不返回子 agent 的回答正文」 | `AgentNode` 压根没有装正文的字段 | ✅ |
| 「正文会在**那次 spawn 调用的结果里**回到你这里」 | 前台 spawn ✅（`subtree::harvest_slots` 回写 spawn 槽）；**后台 spawn ❌**——`spawn_tool::detach` 只回 `{"agent_id":"root/a1"}`，正文进 stash 等 `collect` | ❌ **对后台子是假话** |
| activity 五个变体的拼法 | 跟 `activity()` 的 `match` 逐字对得上 | ✅ |

**二、`spawn_tool.rs` —— 也不一致，一并改了**（052 写下时 `collect` 还不存在，
053 落地时没回来改这段——跟 status 是同一类漏，只是审计只捞到了 status 那条）。

| 描述里的话 | 实际行为 | 判定 |
|---|---|---|
| 「它的最终回复会作为这次调用的结果回到你这里」（**无条件**写在第一段） | 只对 `background=false` 成立 | ⚠️ 后面那段虽然补了条件，但第一段是句无条件断言 |
| 「background=true 时这次调用**立刻**返回子 agent 的 id」 | `detach` 当场发 `Event::ToolResult`，槽收敛 | ✅ |
| 「用 srv:agent/status 看它在干啥」 | ✅ | ✅ |
| 「它的回答不会自己回到你这里」 | `harvest_detached` 进 stash 不回写父 | ✅ |
| 「**你这一轮结束时它会被拆掉**」 | 只有**没 collect 绑定**的才被拆——`Subtree::take_orphans` 的判据是 `detached && is_live && 没有 collect 绑定`（053 §「孤儿判据第三条」） | ❌ 少了「没领的」这个前提 |
| 「需要它的答案就别开后台，用默认的 background=false」 | **053 之后是反的**：`srv:agent/collect` 就是来领它的答案的 | ❌ **把模型往反方向推** |
| schema 里 `background` 那格 | 只说了「立刻返回 id」，**一个字没提 collect**，模型读 schema 时拿不到「答案要自己去领」这件事 | ❌ 漏 |

**三、`collect_tool.rs` —— 核对过，一致，一字未改。**

| 描述里的话 | 实际行为 | 判定 |
|---|---|---|
| 「领一个你用 background=true 开出去的子 agent 的最终结果」 | 两条命中路都要求 stash 命中或 `is_detached`，都只有后台 spawn 才产生 | ✅ |
| 「已经跑完 → 立刻返回」 | `take_stashed` → `reply::settle` | ✅ |
| 「还在跑 → 这次调用等它跑完再返回（和不带 background 的 spawn 一样会等）」 | `subtree.record(...)` + `Dispatched::Nothing` → 槽 `Pending` → `harvest_slots`。**跟前台 spawn 进的是同一张表**（053 §「collect 的两条路」） | ✅ |
| 「一个子 agent 的结果只能领一次」 | `take_stashed` 的 `remove` 就是消费本身，第二次落第三条路 | ✅ |
| 「本来就不是 background=true 开的 → 返回错误」 | `not_collectable` | ✅ |
| 「（不影响你继续干别的）」 | `reply::refuse` 是 `is_error` 的 tool_result，槽照样收敛 | ✅ |
| 「这一轮结束前没领的会被拆掉、结果丢弃」 | `orphan::reap` + `OrphanFate::Discarded` | ✅ |
| 「先用 status 看谁 Done 就先 collect 谁」 | ✅ | ✅ |
| schema 的 `id` 必填 + 「必须是你的后代」 | `parse` 必填、`is_descendant_of` 闸（红线 10） | ✅ |

唯一**没写进描述**的一格是 `already_awaited`（同一个子被 collect 两次而第一次还没
回来）。这不是假话是省略，而且它有自己的拒绝文本兜底——**没补**：这段每一轮都进
prompt，为一条罕见路多花几十个 token 不划算（029「描述写给模型看」的取舍）。

### 新旧文案

**`status_spec()`**（`status_tool.rs:64-75`）：

```diff
- 它**不返回子 agent 的回答正文**——正文会在那次 spawn 调用的结果里回到你这里。
- 什么时候用：……据此决定后面怎么安排。
+ 它**不返回子 agent 的回答正文**。正文从哪来，取决于你当初怎么 spawn 的：
+ 前台 spawn（缺省的 background=false，那次调用会等）的正文**就是那次 spawn 调用的
+ 结果**；`background=true` 开的那次 spawn 只回了一个 agent_id，它的正文**要用
+ srv:agent/collect 去领**——在这里看到它 Done 不等于你已经拿到答案了，没领的后台子
+ agent 会在你这一轮结束时被拆掉。
+ 什么时候用：……据此决定后面怎么安排（看到谁 Done 就先去 collect 谁）。
```

「**在这里看到它 Done 不等于你已经拿到答案了**」是专门给 status 加的一句：`Done` 是
这个工具唯一会让模型误判「事情办完了」的输出，而这正是 DOC-AUDIT 描述的那条失败路径。

**`spawn_spec()`**（`spawn_tool.rs:58-67`）：

```diff
- 把一件……交给一个新的子 agent 去做。子 agent 并行工作，它的最终回复会作为这次调用
- 的结果回到你这里。
+ 把一件……交给一个新的子 agent 去做。子 agent 并行工作。
  什么时候用：……那比你自己做更慢更贵。
- background=true 时这次调用**立刻**返回子 agent 的 id（不等它干完），你可以接着做
- 别的事、用 srv:agent/status 看它在干啥；但它的回答不会自己回到你这里，而且**你这
- 一轮结束时它会被拆掉**——需要它的答案就别开后台，用默认的 background=false。
+ background=false（缺省）：这次调用**等它干完**，它的最终回复就是这次调用的结果。
+ background=true：这次调用**立刻**只返回一个 agent_id（不等它干完），你可以接着做
+ 别的事、用 srv:agent/status 看它在干啥。**它的回答不会自己回到你这里，必须用
+ srv:agent/collect 把它领回来**；你这一轮结束前没领的会被拆掉、结果丢弃。
```

两种 spawn 从「一句无条件断言 + 一段例外」改成**并列的两格**——模型读的是一张
对照表而不是一句话加一个但书。

**`background` 参数**（`spawn_tool.rs:84`）：

```diff
- true = 不等它干完，这次调用立刻返回它的 id；false（缺省）= 等它干完，它的回答就是
- 这次调用的结果。
+ true = 不等它干完，这次调用立刻只返回它的 agent_id，它的回答不会自己回来，得用
+ srv:agent/collect 领（这一轮结束前没领的会被拆掉）；false（缺省）= 等它干完，
+ 它的回答就是这次调用的结果。
```

### 测试断言的是哪些子串，以及**为什么不逐字断言**

三条测试各住在对应工具既有的 `#[path]` 子文件里（红线 9：`status_tool.rs` 只剩
34 行余量，不往源文件里塞测试），**没有新建文件、没有新的挂载方式**。

| 测试 | 断言的子串 | 守的是哪条事实 |
|---|---|---|
| `status_tool::tests::the_spec_tells_the_model_where_a_childs_answer_actually_comes_from` | `"不返回子 agent 的回答正文"` | status ≠ collect |
| | `"前台"` | 前台那条路（正文从 spawn 槽回来）没被删掉 |
| | `"background=true"` | 两种 spawn 分开说了 |
| | `crate::COLLECT_TOOL`（= `"srv:agent/collect"`） | **后台子的正文只有这一条出路** |
| `spawn_tool::tests::the_background_option_says_the_answer_will_not_come_back_by_itself` | `"background=false"` / `"不会自己回到你这里"` / `crate::COLLECT_TOOL` / `"拆掉"` | 后台四件事：缺省是等的、结果不会自己回来、去哪领、不领的下场 |
| | 参数说明里的 `"不会自己回来"` + `crate::COLLECT_TOOL` | 模型读 schema 时未必回头看长描述，那一格得自己立得住 |
| `collect_tool::tests::the_spec_states_the_facts_the_model_cannot_guess` | `"background=true"` / `"只能领一次"` / `"拆掉"` / `crate::STATUS_TOOL` | 只领后台子、领取即消费、轮末没领就没了、跟 status 的配合 |

**为什么是关键子串而不是整段逐字相等**：这段文案**就是拿来调的**（029：描述写给
模型看，措辞会随真机反馈改）。逐字断言的失败模式是**测试被顺手改掉**——下一个人
改一句措辞，测试红，他把期望字符串一起粘过去，这条测试从此什么都不守，而且看起来
一直是绿的。子串断言分得开「文案变了」和「说法和行为对不上了」：只有后者会红。

**工具名走常量不写字面量**（`crate::COLLECT_TOOL` / `crate::STATUS_TOOL`）：三个描述
互相引用对方的工具名，改名而描述没跟上是这类假话的第二种长法。走常量之后这一种也
自动被守住——描述里那个名字必须跟真实的工具名逐字一致，否则红。

### 突变验证（真实红/绿输出）

**突变一**：把 `status_spec()` 的描述整段改回 051 的旧文案。

```
$ cargo test -p agent-runtime --lib status_tool
test status_tool::tests::the_spec_tells_the_model_where_a_childs_answer_actually_comes_from ... FAILED

thread '...the_spec_tells_the_model_where_a_childs_answer_actually_comes_from' panicked at
crates/agent-runtime/src/status_tool_tests.rs:280:5:
前台那条路（正文从 spawn 槽回来）得说：看一眼你 spawn 出来的子 agent 此刻在干啥。**不阻塞**，当场返回。
返回你子树里每个后代的：id、深度、活动状态、以及它的任务。活动状态是Idle（还没开始）/ Thinking（正在想）/ Working(工具名...)（正在跑这些工具）/ Done（这一轮结束了）/ Failed(原因)（没走完）。
它**不返回子 agent 的回答正文**——正文会在那次 spawn 调用的结果里回到你这里。
什么时候用：你并行拆了几个子任务，想知道谁还在跑、谁已经完了、谁失败了，据此决定后面怎么安排。

test result: FAILED. 19 passed; 1 failed; 0 ignored; 0 measured; 79 filtered out
```

**第一条断言挡在前面，后面的 `collect` 那条有没有在干活看不出来**——所以把前两条
临时注释掉再跑一次（证明红不是只红在一句 `assert!` 上）：

```
thread '...the_spec_tells_the_model_where_a_childs_answer_actually_comes_from' panicked at
crates/agent-runtime/src/status_tool_tests.rs:283:5:
后台子的正文要用 collect 领：看一眼你 spawn 出来的子 agent 此刻在干啥。……
它**不返回子 agent 的回答正文**——正文会在那次 spawn 调用的结果里回到你这里。……

test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 98 filtered out
```

**突变二**：把 `spawn_spec()` 的 `background` 那一段改回 052 的旧文案。

```
$ cargo test -p agent-runtime --lib spawn_tool::tests::the_background
test spawn_tool::tests::the_background_option_says_the_answer_will_not_come_back_by_itself ... FAILED

thread '...the_background_option_says_the_answer_will_not_come_back_by_itself' panicked at
crates/agent-runtime/src/spawn_tool_tests.rs:107:5:
得告诉它去哪领：把一件可以独立完成的子任务交给一个新的子 agent 去做。子 agent 并行工作。
什么时候用：一件事能拆成几块互不依赖、各自要读不少材料的子任务时。不要为一次文件读取或一句话回答开子 agent——那比你自己做更慢更贵。
background=true 时这次调用**立刻**返回子 agent 的 id（不等它干完），你可以接着做别的事、用 srv:agent/status 看它在干啥；但它的回答不会自己回到你这里，而且**你这一轮结束时它会被拆掉**——需要它的答案就别开后台，用默认的 background=false。
上限：agent 树深度最多 3（你在 root 时是 0），每个 agent 最多同时有 8 个活着的直接子 agent。超了这次调用会返回错误，那时请自己收敛（少拆几个，或者自己做）。

test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 98 filtered out
```

三处全部改回新文案后绿（见下面的完整验证）。

### 红线 11：这次改动的**一次性**代价，如实标注

工具描述在每一次请求里都排在最前面（`ToolTableSpec::Full`），所以**改动它 = 所有
既有会话的 prompt 前缀在下一轮全部失效一次**：那一轮不命中前缀缓存，全价（DeepSeek
上是 120 倍那个量级）。

**这是一次性的**：新前缀从下一轮起重新稳定下来，之后照常命中。对比不改的代价——
这段字符串**每一轮**都在花钱说一句对后台子 agent 不成立的话，而且它的失败模式是
「模型信了 → 不调 collect → 轮末被 `orphan::reap` 拆掉、结果丢弃 → 一整棵子树的
token 白烧，测试全绿」。一次前缀失效换掉一个每轮复发的错误信息，账是正的。

**这条红线管的不是「不许改」，是「改完之后仍然逐字节确定」**——描述是一个
`Arc<str>` 常量（`spawn_spec` 那个 `format!` 的两个参数来自 `AgentLimits`，同一份
配置下逐字节相同），没有 `HashMap` 迭代、没有时钟、没有随机，改前改后都满足。

### 没做

- **`collect_tool.rs` 一字未改**——核对下来它跟 053 的实际行为一致（见上表）。
- **没碰行为逻辑**：`observe`/`parse`/`intercept`/`detach` 一行没动。
- **没碰** `agent-server/`（062）、`agent-mcp/`（070）、`agent-tools/`、`tool_table.rs`（062）。
- **没同步 `docs/DOC-AUDIT.md` 的 B 条**（它是审计快照，记的是「当时看到什么」）。

### 验证（前台跑完，真实输出）

```
$ export CARGO_TARGET_DIR=…/scratchpad/target-071
$ cargo test -p agent-runtime
exit=0
binaries=48 passed=226 failed=0 ignored=0

test collect_tool::tests::the_spec_states_the_facts_the_model_cannot_guess ... ok
test spawn_tool::tests::the_background_option_says_the_answer_will_not_come_back_by_itself ... ok
test status_tool::tests::the_spec_tells_the_model_where_a_childs_answer_actually_comes_from ... ok

$ cargo clippy -p agent-runtime --all-targets -- -D warnings
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 10.34s
exit=0

$ bash scripts/check-invariants.sh --all
红线检查通过
规则与理由：docs/INVARIANTS.md
exit=0
```
