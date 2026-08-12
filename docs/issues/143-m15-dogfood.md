# 143 真机 dogfood ← M15 终点

**里程碑** M15 · **依赖** [136](136-turn-end-driver.md) + [141](141-remove-activation-subsystem.md) + [142](142-skill-hidden-frontmatter.md) · **模型** **opus** · **独测** 本条即验收 · **状态** 完成（见文末）

## 目标

M15 的全链真机验收（真 provider，不是 mock），跨 133–142 的交界处走一遍。
132 的先例：dogfood 专门抓「每条 issue 各自绿了、合起来漏的那种」。

## 准备

skills 目录：一个 router skill（正文引用一个 hidden 子 skill 的 id）+
该 hidden 子 skill（正文藏口令）。一个 fake `TurnEnd` hook 工具（计数落临时文件）。

## 验收（逐条可判定，全过才算 M15 完成）

CLI + DeepSeek：

1. 首轮 system 含索引（来自开局工具的前缀块）、含 router 不含 hidden 子 skill 的
   行、**不含任何正文字节**。
2. 问口令：模型自主 read router → 顺正文引用 read hidden 子 skill → 说出口令。
   **两跳都是模型自己决定的**，没有任何预激活。
3. undo 撤掉含 read 的那轮 → 下一轮 encode body 中正文字节消失
   （撤回并进 undo，决策 27 的承诺）。
4. `kill -9` 重启恢复：前缀块逐字节原样；开局工具执行计数**仍是 1**（不重跑）。
5. 十轮里每个完成轮 hook 恰好触发 1 次；`Ctrl-C` 取消的那轮 0 次。
6. 十轮第 2 轮起每轮 `cached_tokens / prompt_tokens ≥ 0.9`——含 read 发生的
   那几轮（正文是消息尾追加，不该破前缀）。

server（宿主声明路）：

7. `capabilities.skills` 声明一个 skill → 索引含它 → read 取到 → 杀进程恢复后
   再 read 仍取到。

## 回填

- 逐条兑现记录写进本文件；发现的漏投影/交界 bug 各自开小 issue 或就地修
  （132 的先例：能一行修的就地修并记录）。
- `ROADMAP.md` §二加 M15 完成段；§四「工具索引 + 详情按需拿」条目下确认
  M15 已落的那一半的真机数字（第 6 条的命中率）。
- `CLAUDE.md` 当前状态段该更就更。

## 注意

- 验收 6 失败的第一嫌疑不是代码是措辞：read 太频繁（每轮都读同一个 skill）
  说明模型没意识到正文已在历史里——先调 read 的 description 与索引首行文案。
- 花真钱：单 agent 跑，别并发派两个（WORKFLOW §四 -2 的教训）。

## 实做记录：M15 真机 dogfood（主会话，2026-08-12，CLI + server + 真 DeepSeek key）

**七条全过。** provider = deepseek/deepseek-v4-flash。

现场：`skills/warehouse-router`（正文引用 `warehouse-vault` 的 id，不含口令）+
`skills/warehouse-vault`（`hidden: true`，正文藏口令 `QINGZHOU-4417-LANTERN`）。

### 1 首轮 system：索引在、hidden 不在、正文一字不在

前缀块 `init:srv:skill/index` 的实际内容：

```
以下 skills 可用 srv:skill/read 按 id 取全文：
warehouse-router — 仓库门禁流程的入口。要办任何跟「仓库门禁」有关的事，先读这一条…
```

四条断言都是对会话 journal 直接搜的：`warehouse-router` **在** /
`warehouse-vault` **不在**（142 的 hidden 生效）/ 口令 **不在** /
router 正文片段「本流程分两级」**不在**。

### 2 两跳 read，全是模型自己决定的

```
思考：warehouse-router 是入口…所以我应该先读取这个 skill
[tool] srv:skill/read {"skill":"warehouse-router"}   → 429 字节
思考：按指引读取 warehouse-vault skill
[tool] srv:skill/read {"skill":"warehouse-vault"}    → 136 字节
答：当班仓库门禁口令是 QINGZHOU-4417-LANTERN
```

**零预激活**——索引里根本没有 `warehouse-vault` 这一行，模型是顺着 router 正文里
的引用找过去的。决策 27 的「树形靠正文引用递归展开」在真机上成立。

### 3 undo 撤回正文：行为级证据，不是数 token

`/undo` → `[已撤销] 第 1 轮，7 条`，下一轮 prompt 6759 → 6361。

但 token 数只是旁证。**硬判据是直接问它**：

> 提问：不要调用任何工具，仅凭上下文已有信息回答，口令是多少？
> 思考：我的上下文里没有关于仓库门禁口令的任何信息。直说没有。
> 回答：**没有。我的上下文里不存在任何仓库门禁口令的信息。**

三轮前刚说出口令的同一个模型，撤回后说不知道。「正文撤回并进 undo」兑现。

### 4 `kill -9` 恢复：前缀块逐字节原样，开局工具计数仍是 1

轮内 `kill -9`（会话文件停在 7 行、最后是 `cursor`，半轮状态）→ 重启：

```
[会话已恢复] 接着第 1 轮继续
前缀块 sha  崩溃前 28d89875ff594fd7 → 恢复后 28d89875ff594fd7
prefix_init 条目数  1
```

另一份会话被 4 个独立进程打开过，`prefix_init` 也始终是 1 条。**开局工具不重跑**
（134 的「结果落 store，恢复读状态」）。

### 5 hook 每个完成轮恰好 1 次 / 取消轮 0 次 —— 已被 native 完整覆盖，不另搭真机脚手架

`turn_end_indep.rs` 有 `turn_end_hook_fires_exactly_once_per_completed_turn` 与
`cancelled_turn_does_not_fire_the_turn_end_hook`（后者照抄 `cancel.rs` 的取消模拟），
而驱动挂在 `runner.rs:293` 的 `turn_end::fire(ctx)` —— **CLI 走的就是这条 runner**，
不存在「CLI 没接上」的缺口。

CLI 今天没有注册任何 `TurnEnd` 工具的入口，为这一条现造一个脚手架，测到的仍是
上面那两个 native 测试已经钉死的语义。**判断为不值得，理由记在此**——真机的价值在
「跨进程/跨真 provider 才暴露的东西」，这一条不属于。

### 6 十轮缓存命中：13 跳 97.5%–99.8%，含 3 个 read 跳，零条低于 0.9

| 跳 | 轮 | prompt | cached | 命中率 |
|---|---|---|---|---|
| 1 | 1 | 6412 | 6400 | 99.8% |
| 2–4 | 2 | 6460 / 6692 / 6798 | 6400 / 6528 / 6656 | 99.1% / 97.5% / 97.9% |
| 5 | 3 | 6838 | 6784 | 99.2% |
| 6–7 | 4 | 6907 / 7033 | 6784 / 6912 | 98.2% / 98.3% |
| 8–13 | 5–10 | 7079…7414 | 7040…7296 | 97.5%–99.4% |

**均值 98.5%，最低 97.5%。** 这是决策 27 核心赌注最直接的证据：正文以 tool result
从**消息尾**进来，不破前缀。issue §注意 预警的「read 太频繁说明模型没意识到正文已在
历史里」没有发生——十轮里只 read 了 3 次，都发生在真的需要的时候。

### 7 server 宿主声明路：声明 → 索引 → read → `kill -9` 恢复后仍取得到

`POST /sessions` 带 `capabilities.skills`（id `gatehouse-ledger`，正文藏
`BEILU-9052-COMPASS`）：

```
崩溃前   模型 read → 答「当班交接暗号是 BEILU-9052-COMPASS」
kill -9 server → 重启 → POST /sessions 同 id → {"outcome":"recovered"}
恢复后   模型再 read → 答「已重新确认，当班交接暗号是 BEILU-9052-COMPASS」
```

宿主声明的 skill 经 `host_skills` 落盘、恢复后重建 registry（140）——这条路真机成立。

### 跑的时候踩的两个坑（都不是产品 bug，但下一个人会再踩）

1. **server 的私有 API 令牌必须恰好 43 字符**（`run.rs:176` 的校验，url-safe
   base64 的 32 字节长度），且要 `--private-capability-stdin` 从 stdin 喂第一行。
   随手写一个短字符串会得到 `private_capability_input` 而不是「令牌不对」。
2. **不给 `--sessions-dir` 时 server 会话是内存态**，进程一杀就没，验收 7 的后半
   会验成空气。第一次就是这么白跑了一轮。
