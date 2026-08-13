# 157 wasm 宿主同路：页面声明开局块

**里程碑** M17 · **依赖** [155](155-with-host-prefix.md) + [156](156-server-prefix-declaration.md) + [164](164-wasm-skills-declaration.md)（地基） · **模型** 主会话前台（原计划 sonnet） · **独测** — · **状态** 完成（见文末，2026-08-13）

## 为什么后置

开工勘查（2026-08-12）发现：**HEAD 上的 `agent-wasm` 还没有任何 capabilities
声明路**——`capabilities.rs`、`config.rs` 的 `with_declared_capabilities`、
`assemble.rs` 的 `record_capabilities`/`run_session_start` 接线，全部是另一会话
**未提交的在飞工作**（主检出的脏区）。在它落地前做本条 = 在未合并的地基上盖楼，
且必然冲突。**M17 以 158（server 形态真机）收口，本条等地基合并后作为补做**。

## 目标（地基落地后生效）

浏览器宿主（页面声明）同一条路：页面 capabilities 加 `prefix`，建时落店、
恢复读回、装配尾接 `with_host_prefix`（155）。若届时 wasm 已接
`run_session_start`，合成条目自动被它跑到，驱动零改动。

## 做什么

1. wasm 侧 capabilities 解析加 `prefix` 字段，校验规则**逐条对齐 156 的
   validate**（前缀、内部重名、与 tools 重名、空 text）——两边规则出入即 bug，
   测试逐条镜像。
2. 建时 `declare_host_prefix`（154），恢复时 `host_prefix()` 读回，装配链尾接
   `with_host_prefix`。
3. `scripts/build-wasm.sh --dev` 过。

## 验收

- 声明一块 → 新建会话首轮料的 system 段含 `init:<name>` 块且在内置块之后。
- 恢复路：不重跑、值从店回放。
- 不声明的页面：装配产物与本条落地前逐字节相同。
- 校验拒绝路：坏前缀/空 text 当场拒，错误文本点名。

## 注意

- 开工前先确认另一会话的 agent-wasm 工作**已合并**且 `git status` 无交集脏区；
  仍有在飞就继续等，不要抢改。
- 本条的落点符号（模块名、函数名）以**届时合并后的真实代码**为准，
  上面写的名字来自在飞工作的一瞥，可能会变——先 `ls` + `grep` 再动手
  （WORKFLOW §四第 0 步）。

## 实做记录（主会话前台，2026-08-13）

**地基的来路变了**：等的那个「第三个会话」已不在（用户确认），其在飞工作由本会话
认领收尾为 [164](164-wasm-skills-declaration.md)（提交 `45554e9`）。本条踩着它补做。

**落点**：

- `capabilities.rs`：`prefix` 字段解析 + 四条校验，判定与 server 的
  `validate_prefix.rs` **一字不差**（前缀 `web:`/`desk:`；本体白名单非空、
  `[A-Za-z0-9_/-]`、全名 ≤128；声明内/与 tools 重名拒；空 text 拒），文案贴宿主。
  单测覆盖六条拒绝路 + `"web:/"` 合法（两边一致的另一半证据）。
- `config.rs`：`declared_prefix` 第三样声明，同一条「建宿主定死」性质；
  `has_declared_capabilities` 扩到三样。
- `tools.rs`：`browser_tool_table` 链尾接 `.with_host_prefix(..)`（155 表尾约定）。
- `assemble.rs`：`capabilities_for_session` 三元组（恢复只认 journal / 新会话用
  构造配置）；`record_capabilities` 补 `declare_host_prefix`。既有单测扩到
  prefix，并钉「合成条目不进 `declares()`」。

**验证**：`cargo check --tests --target wasm32-unknown-unknown` 绿、
`build-wasm.sh --dev` 绿、`check-invariants --all` 退出 0、全文件 ≤300 行。

**真机浏览器 smoke（chrome + 真 DeepSeek，与 164 合验，四钉全进）**：

1. 声明 1 skill + 1 prefix 块（藏口令 `QIANTANG-3352-HARBOR`）→ 新会话第一问
   零工具答出口令，思考原文 "The briefing says it's QIANTANG-3352-HARBOR"；
   模型面表 = 3 内建 + `srv:skill/read`，合成的开局块条目不在其中。
2. 「读 ops-manual 的暗号」→ 模型自主调 `srv:skill/read`，正文以 tool_result
   回来，暗号 `LUOXIA-8896-PAGODA` 原样引用（164 的链路）。
3. **刷新页面 + 零声明宿主 + 同会话 id**：恢复后再问口令仍答
   `QIANTANG-3352-HARBOR`，思考自述「来自 system prompt 里的值班简报」——
   那个块只可能来自 journal 回放，空配置没有抹掉历史（决策 31 的恢复承诺）。
4. 恢复后的会话表仍含 `srv:skill/read`（第二次 read 成功再引暗号）——
   表从 journal 重建完整。

**一个如实记录的显示性差异**：`AgentHost::toolTableJson()` 是宿主级常量
（构造期按当前配置算），恢复出的会话按 journal 另建表——零声明宿主恢复老会话时
`toolTableJson` 不含 `srv:skill/read` 而会话实际有。164 的原作者已把它的文档改成
「这个宿主给**新会话**的工具表」，语义自洽，不是 bug；页面要看恢复会话的真实
能力面，看行为（或将来加会话级读口，等真实需求）。
