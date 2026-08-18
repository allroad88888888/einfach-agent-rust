# 203 文档同步：把「声明可逆性」的说法从五份文档里换掉 ← M19 终点

**里程碑** M19 · **依赖** [201](201-runtime-undo-fn-delivery.md) + [202](202-host-mcp-undo-none.md) · **模型** sonnet · **独测** — · **状态** 完成（见文末，2026-08-18）

## 目标

199–202 之后，仓里有**五份文档**还在按「可逆性是一个声明的等级」写。它们不改，下一个
人（或下一个会话）就会照着过期的说法去设计——这正是 `CLAUDE.md` §当前状态那句
「看到任何提『激活 skill』的文档，那是过期的」在防的事。

## 做什么

### 1. `docs/EXTENSIONS.md`

- §一 交付物表：`with_tool` 的三元从 **spec + 可逆性 + 执行体** 改成 **spec + 执行体**，
  可逆性由执行体返回值决定。
- §「可逆性：**没有缺省**」整段重写。**保留那段的judgement**（拿不准就别交函数、
  举证责任在包作者、判错代价不对称），**换掉落点**：从「少给一个枚举不编译」变成
  「交不交函数」。原来那句「不是『不填就 `Irreversible`』——缺省值等于告诉作者
  『这件事可以不想』」在新形状下更强了：**不交函数就是挡 undo，作者想躲也躲不掉。**
- §「可逆性存哪儿：复用注入映射」整段**删掉**——`host_reversibility` 那张映射在 202
  之后不再决定行为。
- §四 手套的「能与不能」加两条：还原函数**不能**拿 `&Session`（201 §注意），
  timed 钩子**没有**还原函数（副作用不进 command log，另一件事）。
- §五 教材 `ext:stats` 跟着 201 的改动更新。

### 2. `docs/TOOLS.md`

- §可逆性判据表整段重写。今天那张表教人「怎么给一个工具定等级」，之后要教
  「怎么写还原函数，以及什么时候该不写」。
- §「位置与可逆性不是 spec 的字段，是宿主现算出来的」那段：`reversibility_of` 的
  地位从「行为依据」降成「显示标签的来源」。
- **顺手修掉一处已知过期**：`docs/TOOLS.md:34-37` 说「宿主声明的等级会被
  `reversibility_of` 的兜底盖成 `Irreversible`」——062 的三级优先级早就修掉了
  （`tool_table.rs:265`），这段话在 199 勘查时发现是陈的。

### 3. `docs/HOST-CAPABILITIES.md` §五

整节重写。今天写的是「宿主愿意声明就用，不声明落保守」，之后是：
**声明只作自我描述，不影响 undo 是否停下；宿主工具一律停下，因为它交不出还原函数。**
并指出宿主侧还原回调是将来的事，不在本里程碑。

### 4. `docs/MCP.md` §枢纽

「可逆性不能再从名字推」那节的结论不变（可逆性是 per-tool 元数据），但要补一句：
**`readOnlyHint` 从此只影响显示，不影响 undo**——MCP 协议里没有撤销这个概念，
server 交不出函数（202 §2）。

### 5. `docs/INVARIANTS.md`

红线 6（在飞 effect 必须带 epoch）的说明里补一句还原钩子的位置：**钩子跑在 bump 世代
之后、`apply_prev` 之前**（200 §注意）。这不是新红线，是既有红线在新路径上的落点。

### 6. `docs/ROADMAP.md`

- §一 补上决策 199（十条，见 [199](199-reversibility-as-delivery-decision.md)）。
- §二 加 M19 的实做记录。
- §四 未决问题加一条：**宿主侧还原回调**（第二步，等真实宿主要它再开）。

### 7. 盘点期发现的四处（本 issue 一并做，别丢）

写这条 issue 时点名了五份文档的主要段落，实际盘点又查出四处**同样过期、但原文没点名**
的地方。它们跟上面六节同一条理由，一起做：

- **`docs/EXTENSIONS.md` §六 对比表第 268 行**——「可逆性从哪来」那一行**三栏全过期**
  （扩展包 / MCP / 宿主各一栏）。这一行是全文档最容易被当成速查表引用的地方。
- **`docs/TOOLS.md` §MCP「reversibility 等级从哪来」第 339–351 行**——与 `docs/MCP.md`
  §翻译规则**内容重复**且同样过期。两处都要改，或者趁这次把 TOOLS.md 那份改成链过去
  （**倾向后者**：同一条规则两份正文正是本仓一贯拒绝的「第二份要维护的真相」）。
- **`docs/HOST-CAPABILITIES.md` §四协议示例第 160 行**——`"reversibility": "pure"` 后面
  跟着 `// 可选，见 §五`。§五 重写之后读者会**先看到示例、以为 `pure` 有实际效果**，
  几行之后才在 §五 读到反转。§五 开头补一行加粗结论（「声明这个字段不再影响 undo
  是否停下」），或者示例注释里补半句。
- **`docs/MCP.md` §「自查：放错地方的症状」表第 154–164 行**——按该表既有风格加一行：
  把 `readOnlyHint=true` 当成「不挡 undo」→ 症状是把显示标签当成行为依据 → 正确做法是
  只用于显示。

### 8. 盘点草稿有两处失效，动手前先重核行号

盘点是在 199 §一 还写着 `Option<UndoFn>`（两态）时做的。review 时已改成三态
`Aftermath`（`Nothing` / `Undo(f)` / `Irreversible`，与落盘的 `Undoability` 1:1）。
草稿里凡是写「交 `Some`/交 `None`」的地方**都要按三态重写**——尤其 EXTENSIONS.md
§一、§「可逆性：没有缺省」、§六对比表这三处，它们正是要教人怎么填的地方，写成两态
就是教错。

**第二处失效：200 落地时已经先改了三段文档**（`STATE-MODEL.md` §「Command log」的
`EntryMeta` 代码块与「唯一落盘依据」那段、`STATE-MODEL.en.md` 同款、`docs/TOOLS.md`
§屏障那段）。那三段被 200 的代码直接改成了假话，**当场改是对的**，但盘点草稿是在那之前
做的：`TOOLS.md` 的行号已经整体后移，`STATE-MODEL` 两份根本不在草稿的五份清单里。
动手前先跑一遍 `git log -p -- docs/` 看 200 已经改过哪些，**别把已经对的段落再改一遍**。

## 验收

- 全仓 grep `Reversibility::Reversible` /「声明可逆性」/「可逆性等级」/`Option<UndoFn>`，
  剩下的每一处都要么在**显示**语境里，要么带着「这是 199 之前的历史」的标注。
- 五份文档里没有任何一处还在教人「给工具填一个可逆性等级来决定 undo 挡不挡」。
- `docs/TOOLS.md:34-37` 那处过期描述已修正。
- 三门禁全绿（`cargo test --workspace` / `check-invariants --all` / `build-wasm`）。

## 注意

- **英译版**（`INVARIANTS.en.md`）也要跟。中文是权威，但两边并存时只改一边 = 埋一个
  下次会被引用的错误版本。
- 别删历史。199 之前的判据（三档枚举、`readOnlyHint` 翻译规则）**留在原 issue 里**
  （040/042/062/073/076/146–149），那些是当时的真实决策记录。本条只改**现行文档**。
- 别顺手重写 §可逆性判据表里那句「拿不准就 `Irreversible`」的精神。落点变了，
  判据没变——而那句判据是这套东西里最值钱的一句。

## 实做记录（2026-08-18，四个 sonnet agent 按文件切分并跑，review 已过）

五道门禁全绿：`cargo test --workspace` 2161 passed / 0 failed；`check-invariants.sh --all`
退出码 0，13 条红线 9 提示与基线逐条相同；`build-wasm.sh` 绿；`pnpm -r typecheck` 绿；
`cargo clippy --workspace --all-targets -- -D warnings` 干净。

**原清单 18 项里有 6 项已经被 201/202 顺手做掉**（EXTENSIONS §一、§「可逆性：没有缺省」、
§五，HOST-CAPABILITIES §五，ROADMAP §一 决策 34、§四 未决问题）。§8 那条「动手前先重核行号」
是对的，而且比预计更值——盘点草稿写于 199 还是两态时期，行号也已整体后移。

### §五 那一节是**假完成**——本次真正的收获

202 落地时 §「五、可逆性」看起来「已整节重写」，但它写的是 **202 撞额度前的 B 版初稿**：
标题写「宿主工具**一律挡** undo」、表里 `pure` 那行是「不挡 → **挡**」。而决策 34 review
时改成的是「承诺挡、事实不挡」，代码里 `undo_promise.rs` 明写 `Reversibility::Pure => false`。
**文档与代码正好相反，且看起来是完整的一节。**

这跟 202 那条没来得及反转的测试断言（`a_host_tool_declaring_pure_blocks_undo_too`）
是同一次中断留下的两个尾巴，测试那条被 `cargo test` 逼出来了，文档这条没有任何东西会报错。
主会话上一轮盘点时也**漏了它**：grep 用的是「可逆/reversib」，而表里那行是
`| `pure` | 不挡 | **挡** |`——一个模式都不命中。

**教训**：一节文档「已重写」不等于「重写对了」。判断依据只能是**拿代码的当前行为逐格核**，
不能靠 grep 关键字，更不能靠上一轮的完成记录。

### 盘点清单外另修的四处

1. **`docs/EXTENSIONS.md` §四 的签名代码块还停在旧的** `Result<Arc<str>, Arc<str>>`——
   §一 改了、§四 漏了，而 §四 正是教人怎么写扩展执行体的地方。
2. **`docs/EXTENSIONS.md` §四 补了 `Err` 的语义警告**：`Err` 是「没碰」不是「失败」。
   碰了一半才失败的调用要交 `Ok((失败说明, Aftermath::Irreversible))`。
   模块文档 `session_tool_ext.rs:66-67` 早有这句，文档侧漏了——用 `Err` 报这种失败
   等于告诉账本「外部世界干净」。
3. **两个死符号**：`docs/HOST-CAPABILITIES.md` 与 `crates/agent-runtime/src/undo_promise.rs:43`
   的 `mark_irreversible`（201 已改名 `mark_no_undo`）、HOST-CAPABILITIES 的
   `repo_cannot_compensate`（202 已改名 `is_unkeepable_promise`）。
   `crates/agent-tools/tests/it/shell_undo_barrier.rs:6` 同款。
4. **`docs/HOST-CAPABILITIES.md` §一 那笔「文档欠账」删掉**——它记的是「TOOLS.md 画了一个
   代码里不存在的 `ToolDescriptor`」，而 TOOLS.md 今天画的就是三个真结构、还专门有一段
   「没有 `Source` 枚举」。欠账早还清了，且那段自己的描述也过期（可逆性早就是三级查表，
   不是它说的「不查表的自由函数」）。

### 顺手修正的两处 TOOLS.md 陈述

- `:21` 伪码行尾注释 `// …—— undo / 崩溃恢复`：两件事现在都不成立（`is_replayable()` 已删，
  恢复走 `apply_next` 重放 journal 状态值、从不重新执行工具）。
- `:250`「撞上 `Irreversible` 的 entry」混了两层词：entry 上落的是 `Undoability::Blocked`，
  `Irreversible` 是那次调用交回的 `Aftermath`。M19 整件事就是要把这两层分清楚。

### §7-④ 那条自查表建议**本身是过期的**

原文让在 MCP §自查表加一行「把 `readOnlyHint=true` 当成不挡 undo → 错」。但决策 A
（事实/承诺）之后 `true` **确实**落 `StateOnly`、**确实**不挡，照写会得出「错在哪」与
「正确做法」同一个结论。根因同 §8：这条建议写于两态时期。改成真正的陷阱——
**以为 MCP server 能交回还原函数**（「让 server 多加一个补偿工具就不用挡了」）。

这是 203 草稿栽在两态遗留上的第四处，前三处在 199 §一/§六/§七。
