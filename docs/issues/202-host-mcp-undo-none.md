# 202 宿主与 MCP 一律不交还原函数：堵掉「声明可逆但没人补偿」

**里程碑** M19 · **依赖** [200](200-core-undo-hook-path.md) · **模型** sonnet · **独测** ✅ · **状态** 未开始

## 目标

决策 199 第七、八条的落地面。**这是 199 现状清账里那个真实失败场景的堵口**：

1. 宿主声明 `{ "name": "web:crm/draft", "reversibility": "reversible" }`
2. 模型调它，在 CRM 里建了一份草稿
3. 用户 `/undo` → CLI 打印「回退了 3 条目」，**没有任何提示**
4. **草稿还在 CRM 里**

## 做什么

### 1. 宿主工具恒 `Blocked`

`web:` / `desk:` 的执行体在**宿主那边**，还原函数交不回来（199 §七：宿主侧还原回调
是第二步，本里程碑不做）。所以 `dispatch` 走远端第五路时，那条 entry 一律落
`Undoability::Blocked`。

**这是一次行为变更，要如实说**：今天宿主声明 `pure` 或 `reversible` 的工具不挡 undo，
之后**一律挡**。

- 声明 `irreversible`（含不声明的默认）→ 行为不变，本来就挡。
- 声明 `pure` → **行为变了**。但「pure = 重复执行任意次外部世界不变」这句话，对一个
  执行体在浏览器里的工具，我们**没有任何办法验证**——`docs/HOST-CAPABILITIES.md` §五
  说「宿主是企业自己的代码，它说 pure 就按 pure 办」，那条判据在 199 之后收窄成：
  **说了不算，交了才算，而它交不了。**
- 声明 `reversible` → **行为变了，而这正是本 issue 的目的**：从「静默跳过」变成
  「停下来问」。

### 2. MCP 恒 `Blocked`

MCP 协议里**根本没有「撤销」这个概念**，server 不会交函数。`mcp:` 的第四路一律
`Blocked`。

- `readOnlyHint == true`（今天落 `Pure`）→ **行为变了**，从不挡变成挡。
- 其余（今天落 `Irreversible`）→ 不变。

`readOnlyHint` 那一档的取舍要在实做记录里写明：它是**第三方 server 的自我声明**，
决策 22 当初就是因为「机械按名字判会把数据事故开关交给第三方」才落保守；199 只是把
同一条判据推到底——**第三方说的话不能成为「不挡 undo」的依据**。

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
- 声明 `pure` 的宿主工具同样 `Blocked`（行为变更的钉子，防止有人后来"顺手优化"回去）。
- `readOnlyHint: true` 的 MCP 工具 `Blocked`。
- `undo_turn_force()` 能越过它们，一次一条。
- 显示层：宿主声明 `reversible` 时打印出来的那一行**含有「本仓不代为补偿」这层意思**
  （断言子串，不断言完整文案）。
- `Reversibility` 仍然在协议里、TS 类型不变（`packages/protocol` 的生成物 diff 为空）。
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
