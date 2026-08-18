# 208 `srv:agent/self`：模型看得到自己的账

**里程碑** M20 · **依赖** [204](204-agent-mesh-decision.md)（拍板） · **模型** sonnet · **独测** ✅ · **状态** 待做

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
