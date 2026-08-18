# 208 `srv:agent/self`：模型看得到自己的账

**里程碑** M20 · **依赖** [204](204-agent-mesh-decision.md)（拍板） · **模型** sonnet · **独测** ✅ · **状态** ✅ 完成（2026-08-18）

## 目标

决策 204 §三 的前半：**让模型知道自己还剩几轮。** 今天它对这些完全瞎着，
所以没法「快没轮次了就收敛输出」。

**不碰任何红线，也不碰 205。** `Private` 的含义是「**别的** agent 读不到」，不是
「自己也读不到」——`visibility.rs:34` 专门澄清过这件事。自读走本 agent 已有的读路，
不经 `peek_agent`（那是跨 agent 的口）。

## 做什么

### 1. 工具

截获位置照 status 同款。**无入参**（自己是谁由截获现场的 `agent` 决定，不给模型
一个能填错的口）。纯读、无 Pending、当场回写、不调 `persist::sync`
（照 `status_tool::intercept` 的既有理由：一条命令都没发）。

可逆性 `Aftermath::Nothing` → `Undoability::StateOnly`。

### 2. 给什么

| 字段 | 从哪读 |
|---|---|
| `id` / `depth` | 截获现场的 `AgentId` |
| `turns_used` / `max_turns` | `Slot::TurnsUsed` / `Slot::MaxTurns`（`read.rs:220` / `:223`） |
| `retries_used` / `max_retries` | 同族两个槽位 |
| 还能开几个子 / 还能往下几层 | `AgentLimits`（决策 32 起是启动参数，`ToolTableSpec::spawn_limits()` 那份） |
| 有几个工具可用 | `Slot::ToolsAllowed` 的**条数**，不列名（名字全在工具表里，重列一遍是纯浪费 token） |
| 上下文压过没有 | `Slot::Summaries` 非空与否 —— **只回布尔，不回内容** |

### 3. 诚实标注必须进工具描述

这一轮回「turns_used=3」，**三轮之后模型读到的还是那个 3，而它早过期了**。
跟时间戳进 prompt 是同一类病：一个看起来永远成立的事实，冻进历史之后就是假的。

描述里要明说「这是你**调用那一刻**的数」，不许写成无时态的断言。

## 验收

- 跑满 `max_turns` 之前调一次、之后再调一次，`turns_used` **确实变了**
  （不是回一份写死的默认值）。
- 子 agent 调 `self`，`depth` 与「还能开几个子」是**它自己的**，不是 root 的。
- 启动参数改过 `--max-agent-depth` / `--max-children` 之后，`self` 回的是**配的那组数**，
  不是 3/8 两个字面量（决策 32：给模型看的和真正拦人的必须是同一组数）。
- **红线 11**：同一状态下连调两次，两段正文**逐字节相同**（不带时间戳、不带调用序号）。
- **不暴露任何别的 agent 的东西**：断言正文里不含任何非本 agent 的 id。
- 恢复之后调 `self`，`turns_used` 是恢复回来的那个值。
- 工具描述里含「调用那一刻」这类时态限定词——**这条写成断言**，它是本 issue 唯一
  防得住「模型把过期数当事实」的东西。
- `cargo test --workspace` 全绿 + `check-invariants --all` 过 + `build-wasm.sh` 绿。

## 注意

- **只读，一个写口都不开。** 「改本 agent 状态」的正确形状是 [209](209-notes-slot.md)
  那个属于模型自己的槽位，不是给这里的任何一格开写口——理由在 204 §三 那张表：
  这里每一格都是别人的账（部署方的 / 父给的 / adapter 的 / 父要读的）。
- **别回 `Slot::ToolsAllowed` 的名单**。工具表本来就在每一轮的 prompt 里，
  再列一遍是纯浪费，而且两份会不一致。
- **别回 `Summaries` 的内容**。摘要正文是压缩边界那一侧的账（`SendPlan` 里的引用指向
  它），把它塞进 tool_result 等于让同一段文字在 prompt 里出现两次。布尔够用。

## 实做记录（2026-08-18）

### 必须用带 `_of` 的读口，这是本条唯一的静默陷阱

`Session::turns_used()` 那一批读的**恒是 root**（`read.rs` 的 `read()` = `slot_of(&self.agent, …)`，
而 `self.agent` 是这棵树的 root，不是「这一步替谁做」）。子 agent 调 `self` 要是走了
那一条，它会拿到 root 的预算当成自己的——链通、值错、不报错。

所以 core 补了 `turns_used_of` / `max_turns_of` / `retries_used_of` / `max_retries_of`
四个 per-agent 读口。**它们不是新的跨 agent 读 API**：这四个槽位站 `Private`，
而 `Private` 的意思是「**别的 agent** 读不到」，宿主不是 agent（`visibility.rs`
的类型文档专门澄清过这件事）。

独立测试 agent 的注入验证正好打在这一格上：把 `turns_used_of` 改成转发给 root →
**只有** `self_indep_child_own_turns_used` 变红，另外 7 条全绿（它们都是 root 视角，
这个 bug 在那儿根本不可见）。

### 「上下文压过没有」没有新增 core 读口

`Session::summary_library(agent)` 已经是 `pub`（109），`!…is_empty()` 就够。
`apply_summary.rs` 里那个私有的 `summaries_of` 贴着 300 行天花板，不碰它。

### 独立测试 agent 的写法：**差分**，不解析正文

它读不到 `self_render.rs`，于是没有一条测试去解析「turns_used=3」这样的字段，
而是造两次调用、让它们**只在被测的那一样东西上不同**，断言两段正文不同；
再配上「这些东西绝不许出现」的缺席断言（工具全名、兄弟的 id、摘要正文）。

**这比解析字段更好**，不是将就：措辞是拿来调的，逐字断言只会让下一个改文案的人
顺手把测试一起改掉。这条经验值得下次照抄。

### 它报回来的三点，逐条回应

1. **「工具描述里含时态限定词」那条不在它的清单里** ——属实，那一条我留在了
   `src/self_tool_tests.rs`（`the_spec_says_the_numbers_are_a_snapshot_in_time`）：
   它测的是 `self_spec().description` 这个**静态字符串**，造一个 `Session` 都不需要，
   放进端到端测试是浪费。这是刻意的分工，不是漏派。
2. **「不许改 `src/`」与「注入验证」看似矛盾** ——它自己的解读是对的：禁止的是
   **最终交付物里有 `src/` 的净改动**，注入是一次必须回到零 diff 的临时例外。
   下次派活时把这句话直接写进 prompt。
3. **「还能开几个子」那个数没有单独的差分用例** ——如实记下来的缺口。身份替换那次
   注入确实整份覆盖到了（含 depth 与子数），但没有单独一条钉住那个数字。
   不补：单独造它要一次「同一跳里既 spawn 又 self」的脚本，脆弱度换来的边际收益
   很小，而 `AgentLimits` 那一格已经有 `self_indep_limits_not_hardcoded` 钉着。

### 顺带

- 还掉 207 留的债：`status_spec_tests` 里 `"srv:agent/send"` 字面量换成 `crate::SEND_TOOL`。
- **`with_send()` 从 206 落地起就没有任何宿主调它**——工具存在、截获注册跟着
  `declares()` 走，于是它是死代码，模型连它存在都不知道。这次 CLI 与 server `Full`
  档一起补上 `send`/`self`/`notes`。这类漏只有在「加下一个工具时顺着装配链走一遍」
  才会被发现，grep `with_send` 是查不出来的（它在 `tool_table.rs` 里有定义）。
- 红线 9 两处就地拆分：`command/read.rs`（318 行 → 拆出 `read_ledger.rs`，
  「读一个 agent 的一格」与「读整份状态/整条日志」是两件事）、
  `tool_table.rs`（311 行 → 拆出 `tool_table_agent.rs`，子 agent 一族六档授权）。
