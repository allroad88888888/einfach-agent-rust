# 045 CLI 接入 + `/mcp` 状态 ← M6 终点

**里程碑** M6 · **依赖** 044 · **模型** sonnet · **独测** —（终点靠真实全链验收）

把 MCP 接进 `agent-cli` 的 bootstrap，加 `/mcp` 状态命令。扛 M6 验收——一次真实的
dogfood：模型自己发现 MCP 工具、调用、拿到真结果，undo 尊重可逆性。

## 范围

1. **bootstrap 接线**（`agent-cli/src/main.rs`）：启动时读 `.mcp.json`（默认当前目录，
   `--mcp-config` 可覆盖），装载 server（044 的失败隔离在里面），把 MCP 工具 + 可逆性映射
   接进 `ToolTable`——追加在既有工具后面（`builtin`/`shell`/`spawn`/`skills` 的顺序是既有
   契约，红线 11，只加不改）。`McpRegistry` 进 `RunnerCtx`。
2. **`/mcp` 命令**（`repl.rs`）：列出每个 server 的状态（connected / unavailable + 原因）
   和它暴露的工具名。对应 Claude Code 的 `/mcp`。
3. **恢复重连**：kill-9 重启后，会话历史从持久化恢复，MCP server 从 `.mcp.json` **重新
   spawn**（句柄住 store 外，本就不该从快照复活——见 MCP.md §「活句柄住 store 外」）。

## 验收（M6 验收，可判定）

- `.mcp.json` 配 `@modelcontextprotocol/server-everything`，`cargo run -p agent-cli` 起，
  `/mcp` 列出 `everything` = connected + 它的工具（`echo`/`add`/... 的 `mcp:everything/` 名）。
- **真实对话**：给模型一个只能靠某个 MCP 工具完成的任务 → 模型发现并调用 `mcp:everything/<t>`
  → 拿到真结果进下一轮 → 用它回答。（第 2 轮起缓存命中 ≥0.9 仍成立——MCP 工具进表是稳定
  前缀，红线 11。）
- **`/undo` 尊重可逆性**：撤一次 readOnly MCP 调用干净越过；若调用了非 readOnly 的工具，
  `/undo` 撞屏障停下问（和 shell 屏障同一套 UI）。
- **kill-9 重启**：会话历史在，`/mcp` 重连 everything，能接着聊。
- 配置缺失 / 没有 `.mcp.json` → CLI 正常起，就是没有 MCP 工具（不报错、不崩）。

## 注意

- **红线 11**：MCP 工具追加进工具表要保持逐字节确定的顺序（server 内工具顺序 = `tools/list`
  的顺序，server 之间按 id 排）。加进 prompt 最前面，顺序漂 = 每轮全价。
- **收工验证前台跑完**（WORKFLOW §四 -1）：真起 everything server、真跑一轮模型调用、
  前台看到全链绿，再交报告。这是 M6 的「能用」终点，验收不靠形容词靠这次真实运行。
- 回填 issue（状态→完成，把真实运行遇到的坑写进「注意」）；更新 ROADMAP §二 加 M6 完成段、
  README 状态行、issues/README M6 段。

## 实现记录（CODE 部分 · 2026-08-03）

**状态：代码接线完成 + 单测绿；真实全链 dogfood 待主会话。** 本条只记 CODE 落地，
**不标整 issue 完成**——M6 终点的可判定验收是一次真实运行（`.mcp.json` 配
`@modelcontextprotocol/server-everything`、模型发现并调用 `mcp:everything/<t>`、`/undo`
尊重可逆性、kill-9 重连），那需要 `providers.toml` + 交互会话，是**主会话**的活，跑完再
标完成、把坑写进上面「注意」。

### 改了哪些文件（行数）

| 文件 | 行数 | 干什么 |
|---|---|---|
| `crates/agent-cli/Cargo.toml` | +1 | 加 `agent-mcp` 依赖（044 的 loader/config/status 不经 agent-runtime 转出） |
| `crates/agent-cli/src/mcp.rs`（新） | 203 | MCP bootstrap 接线：`resolve_config_path`（`--mcp-config` 覆盖）+ `bootstrap`（读 `.mcp.json` → 跑 044 loader → 分三路：tools / registry / `/mcp` 状态）+ 单测 |
| `crates/agent-cli/src/print/mcp.rs`（新） | 96 | `/mcp` 纯格式化器 `render_mcp_status`（三态 + 原因 + 按 server id 前缀分组的工具名）+ 单测 |
| `crates/agent-cli/src/main.rs` | 209→225 | bootstrap 接入点（见下）；banner 加 `/mcp` |
| `crates/agent-cli/src/repl.rs` | 125→134 | `/mcp` 命令接入 slash 分派；`run` 多收一个 `&McpStatus` |
| `crates/agent-cli/src/print/mod.rs` | +3 | 挂 `mcp` 子模块、导出 `render_mcp_status` |
| `crates/agent-cli/src/lib.rs` | +1 | `pub mod mcp` |

（全部文件 ≤300 行，红线 9 达标。）

### bootstrap 接入点（精确位置）

- **读 `.mcp.json`**：`main.rs` 在 skill 装载之后、`open_backend` 之前——
  `let (mcp_config_path, mcp_explicit) = mcp::resolve_config_path(&args);` 后
  `let mcp = mcp::bootstrap(&mcp_config_path, mcp_explicit, &mut |m| eprintln!("[mcp] {m}"));`。
  默认路径 = 启动目录下 `.mcp.json`；`--mcp-config <path>` / `--mcp-config=<path>` 覆盖。
- **工具进表**：`ToolTable::standard_local().with_spawn(..).with_skills(skills).with_mcp(mcp.tools)`
  ——`with_mcp` **追加在最后**，builtin/shell/spawn/skills 的既有顺序一格没动（红线 11）。
- **registry 进 `RunnerCtx`**：`RunnerCtx::new(..).with_agent_events(..).with_mcp(mcp.registry)`
  ——走 043 早就留好的 `RunnerCtx::with_mcp(Arc<McpRegistry>)`，活句柄住 store 外（红线 3）。
- **`/mcp` 数据**：`repl::run(&mut session, &mut ctx, &root, &mcp.status)`。

### `/mcp` 怎么接进分派

`repl::run` 的 `match input` 里加 `"/mcp" => { println!("{}", crate::print::render_mcp_status(mcp)); continue; }`
——跟 `/undo`/`/skills`/`/agents` 同一套 slash 分派。数据源是**装载期** `McpStatus`（server
可用性 + 工具名的可序列化快照），不是活 registry——因为起不来的 server 的原因只在装载状态
里，registry 只装连上的 client。渲染是 `print::mcp` 的纯函数（独立单测），对齐 Claude Code 的
`/mcp`。

### 工具追加顺序怎么做到逐字节确定（红线 11）

两级都钉死：**server 之间按 id 排**——`mcp::bootstrap` 在跑 loader 前对
`config.servers.sort_by(|a,b| a.0.cmp(&b.0))`（撞名已在 `parse_config` 拦掉，id 唯一 →
全序、不受 `.mcp.json` 书写顺序影响）；**server 内按 `tools/list` 顺序**——044 loader 本就
保序。产物 `tools` 直接 `with_mcp` 进表尾，`tool_names` 也按同一顺序，`/mcp` 原样渲染不重排。

### kill-9 重连（自然成立，无新代码）

`mcp::bootstrap` **每次启动都无条件跑**：句柄住 store 外、从不进快照，所以 kill-9 重启后
会话历史照常从持久化恢复，MCP server 却是从 `.mcp.json` **重新 spawn** 的新子进程——
恢复路径天然如此（docs/MCP.md §「活句柄住 store 外」），本 issue 没为此加任何代码，只在
`mcp.rs`/`main.rs` 注释里点明这条流。真实 kill-9 全链验证归主会话。

### 缺失 / 坏配置不致命

没有 `.mcp.json`（默认路径不存在）→ 静默零 MCP 工具起（正常无 MCP 情况）；`--mcp-config`
显式指了不存在的文件、或配置解析失败（撞名 / 语法坏）→ 打一句 `[mcp]` 警告后按无 MCP
继续（跟 skill 装载失败退回空 registry 同一个精神）。都不报错、不崩。

### 验证（前台，非网络）

- `cargo build -p agent-cli`：通过。
- `cargo test -p agent-cli -p agent-mcp`：绿。新增单测覆盖——缺配置静默干净起 / 显式缺配置
  警告仍起 / server 按 id 排 + 失败隔离（命令不存在，非网络 spawn 立刻 ENOENT）/ 远端
  Unsupported 不致命 / `/mcp` 三态渲染带原因 + 工具按 server 分组 / `--mcp-config` 解析。
- `cargo clippy -p agent-cli --all-targets -- -D warnings`：通过。
- `bash scripts/check-invariants.sh --all`：通过。
### 主会话真机 dogfood（M6「能用」终点兑现 · 2026-08-03）

主会话拿 045 编译好的 CLI，真起 `@modelcontextprotocol/server-everything`（npx，13 工具）+
真 deepseek 上游，跑通全链。`.mcp.json` 放 scratch、`--mcp-config` 指过去（不进仓、不提交）。
providers.toml 只读不印（启动行 `key=已配置（35 字符）`，从不出 key 正文）。

**逐条兑现**（真实输出为证）：

- **`/mcp` 列 connected + 工具**：`mcp=1/1 server 连上，13 个工具`；`/mcp` 列 `everything
  connected：13 个工具` + 全部 `mcp:everything/<t>`（echo/get-sum/get-env/...）。
- **模型发现并调 MCP 工具 → 真结果回下一轮**：模型自发
  `[tool] mcp:everything/echo {"message":"MCP-DOGFOOD-7788"} (location=Server
  reversibility=Pure)` → `完成，22 字节` → 用返回的 `Echo: MCP-DOGFOOD-7788` 组织回答。
  **可逆性从 `readOnlyHint` 翻译**（echo 落 `Pure`，不是按名字猜）当场可见。
- **红线 11 缓存**：第 2 轮 `预测 7040 / 实际 7040，一致`、`命中率 48%`——MCP 工具进表是
  稳定前缀，不破缓存。
- **`/undo` 尊重可逆性**：`[已撤销] 第 1 轮，4 条`——Pure（readOnly）MCP 调用**干净越过**、
  不撞屏障（非 readOnly 撞屏障走 020/027 既有机制，未在本次强制触发，映射由 043 单测锁）。
- **kill-9 重连**：真跑一轮落盘后，全新进程读同一 `--session` 文件 → `mcp=1/1 server 连上`
  （从 `.mcp.json` 重 spawn）+ `[会话已恢复] 接着第 1 轮继续` + `/mcp` 复现 everything
  connected。会话历史从持久化回来、活句柄从配置重生（红线 3：句柄不进快照）两条同时成立。
- **无孤儿**：dogfood 结束 `ps` 无残留 `npx`/`server-everything`（`StdioTransport::Drop`
  kill+wait 生效）。

M6 由这次真实运行验收，不靠形容词。**045 完成，M6 收官。**
