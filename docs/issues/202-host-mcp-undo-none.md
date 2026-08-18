# 202 宿主与 MCP 的承诺兑现不了：堵掉「声明可逆但没人补偿」

**里程碑** M19 · **依赖** [200](200-core-undo-hook-path.md) · **模型** **opus** · **独测** ✅ · **状态** 完成（见文末，2026-08-18）

## 目标

决策 199 第七、八条的落地面。**这是 199 现状清账里那个真实失败场景的堵口**：

1. 宿主声明 `{ "name": "web:crm/draft", "reversibility": "reversible" }`
2. 模型调它，在 CRM 里建了一份草稿
3. 用户 `/undo` → CLI 打印「回退了 3 条目」，**没有任何提示**
4. **草稿还在 CRM 里**

## 做什么

### 1. 宿主工具：**承诺挡，事实不挡**

`web:`/`desk:` 的执行体在**宿主那边**，还原函数交不回来。但「交不出函数」这个理由
只对**承诺**成立——见 199 §七（该节 review 时按本 issue 落地发现的后果修正过）：

| 声明 | 落成 | 为什么 |
|---|---|---|
| `pure` | **`StateOnly`（不挡）** | 声明的是「没碰外部世界」这个**事实**，不需要函数来兑现 |
| `reversible` | **`Blocked`（挡）** | 声明的是「有补偿动作」这个**承诺**，而它结构上交不出那个函数。**这一格是本 issue 的目的** |
| `irreversible` / 未声明 | **`Blocked`（挡）** | 行为不变 |

**只有 `reversible` 那一格是行为变更**：从「静默跳过」变成「停下来问」。

初稿写的是「一律挡」，那会连 `ask_user_question` / `browser_action`（`location_of`
判 `Web` 的标准集裸名）一起挡——它们字面上不可能有副作用，挡住保护不了任何东西。

### 2. MCP：同一条判据，**决策 22 不被反转**

- `readOnlyHint: true` → `Pure` → **不挡**，跟 M6 已发布的行为逐字相同。初稿说
  「跟今天 `readOnlyHint` 缺失落 `Irreversible` 的默认一致，不算倒退」——那句话
  **只对「缺失」那半成立**，对 `true` 那半是实打实的反转，199 从没为它单独论证过。
- 其余（缺失 / `false` / 无 annotations）→ `Irreversible` → 挡。**不变。**
- 声明 `Reversible` 的 MCP 工具 → 挡。`agent_mcp::translate` 今天产不出这一档
  （只产 `Pure`/`Irreversible`），但 `ToolTable::with_mcp` 收得下任意等级，而
  `docs/MCP.md` §翻译规则 留着「除非本地配置显式标注」的口子——**那一格不是占位**。

### 3. `Reversibility` 降级成显示标签（199 §八）

枚举**留着**，协议面 `CapabilityReversibility` 不动（删它是破坏性协议变更），
但它不再是任何行为的依据。落地要求：

- `ToolCallRequest.reversibility` 继续带、CLI 与 Web 继续打印——它仍然是「宿主/server
  对这个工具的自我描述」，有信息量；
- **但显示不许撒谎**。`print/events.rs` 与 `render/tool.ts` 打印宿主/MCP 工具的
  `reversible` 时，要同时表明本仓不会代为补偿。最省事的形状：打印那一行加一个后缀
  （例如 `reversibility=Reversible(声明，本仓不代为补偿)`）。具体文案实现时定，
  **但「只印一个 `Reversible` 就完事」不通过验收**——那正是 199 要修的骗人的那个字。

### 4. `is_replayable()` 删掉

199 §八。理由在 199 现状清账：恢复走 `apply_next` 重放 journal 的状态值，**从不重新
执行工具**，「仅 `Pure` 可安全重放」这条判据永远用不上。删方法，删它的单测断言。

`blocks_undo()` 一并删——它的职责被 200 的 `Undoability` 接走了，留着就是第二份真相。

## 验收

- **199 那个失败场景走一遍**：声明一个 `reversibility: "reversible"` 的 `web:` 工具，
  模型调它，`/undo` → **`Blocked`**（不是静默 `Applied`），成因是 `NoHook`。
- 声明 `pure` 的宿主工具 **`Applied`（不挡）**——**这条是「事实/承诺」那道分界的钉子**：
  跟上一条走同一条远端路、同一个「交不出函数」的事实，结局却相反。
- `readOnlyHint: true` 的 MCP 工具**不挡**（决策 22 原样有效）。
- `undo_turn_force()` 能越过它们，一次一条。
- 显示层：宿主声明 `reversible` 时打印出来的那一行**含有「本仓不代为补偿」这层意思**
  （断言子串，不断言完整文案）。
- `Reversibility` 仍然在协议里、**TS 类型形状**不变（生成物的文档注释会跟着 Rust 侧重生成，那是预期的）。
- `cargo test --workspace` 全绿 + `check-invariants` 过 + `build-wasm` 绿。

## 注意

- **这是一次面向宿主的行为变更**，即便协议字段没动。要在
  `docs/HOST-CAPABILITIES.md` §五 明确写出「声明什么都不再影响 undo 是否停下」，
  并在 [203](203-reversibility-docs-cleanup.md) 里同步 `docs/TOOLS.md` §可逆性、
  `docs/MCP.md` §枢纽。
- 别顺手把 `capabilities.tools[].reversibility` 改成 400 拒绝 `"reversible"`。
  199 §八 决定留着它作为自我描述；拒绝一个合法字段会让今天能建的会话建不起来，
  而它并没有变得更不安全（现在一律挡，比之前更保守）。
- 别在这条里做宿主侧还原回调。那是第二步，要动协议、要让 `undo_turn()` 变异步、
  还要定义还原失败怎么收场——等有真实宿主要它再开 issue。

## 实做记录（2026-08-17）

三门禁全绿：`cargo test --workspace` **2149 passed / 0 failed**（200 收工时是 2142，
+3 集成 +5 单测 −1 删掉的 `reversibility_predicates_exhaustive`）；
`check-invariants --all` 退出码 0，13 条红线 9 提示**与基线逐条相同**（本次新增/改动
的文件一个都没被点名）；`build-wasm.sh` 绿。另跑了 CI 的另外两道：
`cargo clippy --workspace --all-targets -- -D warnings` 干净、`pnpm -r typecheck` 两个包
都 Done。

**主验收（199 现状清账那个失败场景）真的走了一遍**，`/undo` 拿到的是
`Blocked { entries: 1, barrier_seq: 2, cause: NoHook }`，不是静默 `Applied`。
夹具从**真的声明 JSON** 进（`host_tools_from_declaration` 收
`{"name":"web:crm/draft","reversibility":"reversible"}`，正是场景第一句话的字面量），
跑一轮真 loop（假 SSE server 扮 provider）、经 `resolve_remote_tool` 回传结果，
再 `session.undo_turn()`——不手搓 entry，否则「标记在派发那一刻」这条时序就没被验到，
而它正是 `dispatch` 那两处改动的全部内容。

**反向注入验过**：把 `dispatch` 那两处 `mark_irreversible` 撤掉，
`host_tool_undo_none` 三条 + `mcp_undo_barrier` 的 readOnly 那条**全红**，而
`non_read_only_mcp_result_gets_a_barrier…`（199 之前就有的那条）**仍然绿**——
说明新加的断言钉的确实是新行为，不是把旧路径重测一遍。

### 落点：两处显式标记，不动那个五路共用的公共块

`dispatch.rs:129-133` 那个 `if matches!(request.reversibility, Irreversible)` 块在
**五条路分流之前**，截获路也共用它（201 正在改那一支）。所以 202 **没碰它**，
而是在 MCP 第四路和远端第五路各自的 `if` 分支里显式 `session.mark_irreversible`。

**新增的 `mark_irreversible` 调用点恰好两处，都在 `dispatch.rs`**：MCP 第四路的
`if tool.starts_with("mcp:") && table_declared` 分支里一处，远端第五路的
`if request.location.is_remote() && table_declared` 分支里一处。改完这个文件里共
三处（第三处是分流前那个公共块，本次一个字没动）。201 把这个方法改名成
`mark_no_undo` 时，本 issue 带来的就是这两处。

两个理由，第二个更重要：

1. 不改公共块就不会把 201 的截获路一起改掉；
2. **「哪一格挡」这件事要在代码上一眼看得见**。靠公共块「碰巧算出
   `Irreversible`」是行不通的——`readOnlyHint: true` 的 MCP 工具和声明 `pure` 的
   宿主工具在那里算出的正是「不标记」，那恰恰是本 issue 要修的 bug。

标记时机沿用公共块那段注释的理由：**派发那一刻**，不是结果落地才回头看。
宿主执行到一半、回传永远不来时，日志里也得有这次调用的屏障位。

### `repo_cannot_compensate`：行为与文案共用一个判据

新文件 `crates/agent-runtime/src/undo_compensation.rs`，一个函数
`repo_cannot_compensate(&ToolCallRequest) -> bool`：`location.is_remote()` 或者名字
`mcp:` 开头。**判据只看名字与位置，不看 `request.reversibility`**——那正是 199 拆掉
的那条路。

单独成一个函数是因为它有**两个必须给同一个答案的消费者**：`dispatch` 的两条路
（行为）与 CLI/Web 的工具卡片（文案）。两处各写一遍 `is_remote() || starts_with("mcp:")`
就是第二份真相，而它漂掉的症状正是本 issue 要修的那个——行为改了、显示没跟上，
用户读到的仍然是「Reversible」。`dispatch` 两处各带一个 `debug_assert!` 钉住这一致性。

MCP 为什么要另判一次前缀而不是靠 `location`：`mcp:` 的 `location` 是
`Location::Server`（子进程往返在宿主本地跑完，不需要远端回传，043 的裁决），
可它的**执行体**在 server 那边——「在不在本进程」和「谁执行的」在这一路上分叉了。

### `readOnlyHint` 那一档的取舍（issue §2 要求写明）

`agent_mcp::translate` 的映射**没删**：`readOnlyHint: true` 仍然翻成
`Reversibility::Pure`。变的是它不再决定任何行为。

理由：决策 22 当初落保守，是因为「机械按名字判会把数据事故的开关交给第三方」。
199 只是把同一条判据推到底——**第三方说的话不能成为「不挡 undo」的依据**，
能成为依据的只有一个交回来的函数，而 MCP 协议里没有这个通道。所以它不是「不信
server」，是「就算它说的是真话也交不出函数」。留着翻译是因为那个自我描述有信息量
（哪些工具 server 自己认为是只读的），而 199 §八 决定 `Reversibility` 留作显示标签。

### 显示：两处，都断言子串不断言完整文案

- CLI：`print/event_text.rs` 新增 `describe_reversibility`，`events.rs` 的
  `[tool]` 那一行改用它。宿主/MCP 工具渲染成 `Reversible（声明，本仓不代为补偿）`，
  本进程内的工具照旧只印 `Pure`/`Irreversible`（那句免责声明对它们是假话，而且
  每行都挂会把真正需要注意的两类淹掉）。
- Web：`render/tool.ts` 的 `metaLine` 同款，判据在 TS 里重写了一遍（一边 Rust
  一边 TS，没有第三条路），两边注释互相指向同一条决策。

### 三处「不改就是假话」的注释（issue 没列）

- `dispatch.rs` MCP 那段前言原文「readOnly 的 MCP 工具落 `Pure` 无屏障」——当场变假；
- `agent-wasm/src/tools.rs` 三条内建工具的「所以 `Pure`——`/undo` 撞上它们不用停下来问」
  ——它们是 `web:` 工具，现在会停；
- `agent-tools/tests/it/shell_undo_barrier.rs` 模块文档引用了被删的
  `Reversibility::blocks_undo()`。

还顺手改了 `packages/web/src/mcp/translate.ts` 的模块文档（浏览器自己连的 MCP 翻成
`web:mcp-…` 声明，同样只剩显示意义）与 `agent_mcp::translate` 的模块文档。

### 偏离 issue 的一处：`packages/protocol` 的生成物**有 diff**

issue 验收写着「`packages/protocol` 的生成物 diff 为空」。实际有一个文件变了：
`packages/protocol/src/generated/Reversibility.ts`。

- **变的只有文档注释**，`export type Reversibility = "Pure" | "Reversible" | "Irreversible";`
  这一行**一个字节没动**（`git diff` 里没有任何 `export type` 的增删行）。
- 原因是 `Reversibility` 的 Rust 文档原文写着「**决定 undo 能不能越过它、崩溃恢复时
  能不能重发**」——那两句话逐字对应着本 issue 删掉的 `blocks_undo()` 与
  `is_replayable()`。删了方法留着这段话，就是在类型定义处留一份第二真相；而 ts-rs
  把 `///` 一并导出，改了不重新生成 `cargo test -p agent-server --features ts` 当场红。
- 结论：这不是「加了协议字段」那种越界，是文档同步的必然代价。**协议形状零变更**，
  `fixtures/events.json` 与其余生成文件 diff 为空。

**review 裁决**（2026-08-17）：接受，不回退——「生成物不该有 diff」那句下得过严，
**以类型形状为准**。同一次 review 也接受了新文件 `undo_compensation.rs`（issue 原文
没有它）：那个判据有两个必须给同一答案的消费者，各写一遍就是第二份真相，而「行为
改了文案没跟上、用户看到的仍然是 `Reversible`」正是 199 现状清账里骗人的那个字。

### 边界：没碰 201 的范围

`ExtensionPack` / `SessionToolFn` 的签名、截获注册表那条路、`dispatch.rs` 的截获分支、
`session_tool_ext.rs:82` 的那句 `mark_irreversible`——一处都没动。
`docs/TOOLS.md` §可逆性 与 `docs/MCP.md` §枢纽 留给 [203](203-reversibility-docs-cleanup.md)
（本 issue §注意 的分工）；`HOST-CAPABILITIES.md` §五 按 §注意 的要求在这里就重写了。


## 修正记录（2026-08-18，review 期）

**实现先按 199 §七 的初稿「一律挡」做完了，然后被自己的落地后果推翻。** 执行方在
报告末尾如实记了一条连带：`ask_user_question` / `browser_action` / `save_file`
（`location_of` 判 `Web` 的标准集裸名）与 wasm 宿主三条内建**也会从此挡 undo**。

那条连带就是反证：`ask_user_question` 字面上不可能有副作用，挡住它保护不了任何东西，
只是让「模型问了一句话」那一轮撤不掉。顺着查下去发现 §七 的理由（「还原函数在对方
进程里，交不回来」）**只对 `Undo(f)` 成立，对 `Nothing` 不成立**——`Nothing` 本来就
不需要函数。用户拍板改判据，199 §七 与 ROADMAP 决策 34 同步重写。

落地的四处改动：判据模块 `undo_compensation.rs` → **`undo_promise.rs`**
（`repo_cannot_compensate` → `is_unkeepable_promise`，穷举 `match` 三格而不是
`matches!`——`Reversibility` 哪天加第四档，编译器会在那里逼一个决定）；`dispatch`
两条分支的标记改成有条件；显示层只给 `reversible` 挂「本仓不代为补偿」（`pure`
没承诺补偿，没什么可不代的）；`mcp_undo_barrier.rs` 的 readOnly 断言与
`agent_mcp::translate` 模块文档改回决策 22 的行为。

**这是同一个「两态合并」错误在 199 里的第二次**（§一 是第一次、§六 是第三次），
三次都是执行的人撞到具体后果才浮出来，写决策和 review 时都没看出来。
