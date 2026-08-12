# 162 `agent-cli` 两个上限 flag

**里程碑** M18 · **依赖** [159](159-agent-limits-config-decision.md)（拍板选 A 才做）
· **模型** sonnet · **独测** ✅ · **状态** 完成（2026-08-12）

## 目标

CLI 侧同款：`agent-cli --max-agent-depth <n> --max-children <n>`。跟
[161](161-server-bin-limits-flags.md) 无依赖，可并行开工。

## 现状

- CLI **没有 `Cli` struct**，flag 是一处一个小解析器：`session_path::resolve`
  （`--session`，带 `AGENT_SESSION_PATH` 兜底）、`ext_stats::enabled`
  （`--ext-stats`，纯布尔）。两个都**收 `&[String]` 参数而不是自己读
  `std::env::args()`**——测试要能喂夹具参数，照做。
- `main.rs:186`：`.with_spawn(session.agent_limits())`——CLI 是**从 session 读**再
  喂给工具表，所以只要在建/恢复 session 之后 `set_agent_limits` 一次，两侧自动
  一致，不需要像 server 那样两处传。**这个顺序不能倒**（先 set 再 `with_spawn`）。
- `main.rs:147` 是 `recover` 调用点，[160](160-recover-limits-param.md) 之后要把
  同一组数传进去。

## 做什么

1. 新文件 `crates/agent-cli/src/agent_limits.rs`（一个文件一件事，照
   `session_path.rs` 的体量与形状）：`resolve(args: &[String]) -> AgentLimits`，
   认 `--max-agent-depth` / `--max-children` 的两种写法，没给的那一项落
   `AgentLimits::default()` 的对应字段。环境变量兜底按 159 的产出决定要不要
   （`session_path` 有、`ext_stats` 没有，两种先例都在，别默认跟随其中一个）。
2. `main.rs`：解析一次，**建新 session 与 recover 两条路都用同一份**——
   新建路 `Session::new` 后 `set_agent_limits`，恢复路经 160 的入参传进去。
3. CLI 的用法提示（若有 `--help` 文本）补两行。

## 验收

- `resolve` 单测：两种写法都认；只给一个时另一个是默认值；都不给等于
  `AgentLimits::default()`；顺序无关。
- **同一组数走两条路**：同一份参数下，「新建的会话」与「从 jsonl 恢复的会话」的
  `agent_limits()` 相等——这条是 160 在 CLI 侧的落地断言。
- 工具描述里的数字 = `session.agent_limits()`（`with_spawn` 在 `set_agent_limits`
  之后调，顺序颠倒会静默给模型看旧数字，这条断言就是那个顺序的看门狗）。

## 注意

- **红线 11**：`main.rs:180-200` 那串 `with_*` 的顺序是既有契约（builtin → shell →
  spawn → status/collect → skills → MCP → vision → 扩展），**只改 spawn 那一项的
  入参，一个 `with_*` 都不许挪位**。
- 一个文件一件事：别把这个解析塞进 `session_path.rs`（那个文件只管会话路径），
  也别塞 `main.rs`。
- 159 若最终选了 B/C（非进程级），这条整个作废——所以**等拍板再开工**。

## 实做记录（2026-08-12，完成）

**新模块 `agent-cli/src/agent_limits.rs`**：`--max-agent-depth`/`--max-children`
两种写法 + env 兜底 + 严格校验 + 启动横幅一行。两层拆分（纯函数 `resolve` /
读 env 的 `from_args_and_environment`）与 server-bin 那份同构，理由同样是
`agent_server::bind` 的并发安全测试那条。

**同一份值喂两条路**（本条的核心）：新建会话走 `Session::set_agent_limits`，恢复走
`recover` 的 `limits` 入参（160）。`with_spawn` 读的仍是 `session.agent_limits()`
**而不是**那个 `limits` 变量——两者此刻相等，但真正该跟工具描述对齐的是会话手上那
一份，从会话读就不会有第二个真相。顺序（先 set 再 `with_spawn`）在代码注释里钉住了。

**测试** 8 条：两种写法 / 默认档 / 部分覆盖不连坐 / 坏值三种（abc、0、缺值）/
env 兜底 + 命令行优先 / env 坏值点名两种写法 / 横幅区分「默认」与「配过」。

## 拆分：`main.rs` 顶破 300，本次一并拆（红线 9）

改动把 `main.rs` 从 295 推到 309。按硬规则「本次改动会顶破上限 → 拆分就是本次改动
的一部分」，抽出 **`agent-cli/src/tool_table.rs`**（79 行）：工具表的组成与顺序
（`builtin → shell → spawn → status/collect → skills → MCP → vision → 扩展`）。

抽这一段而不是别的，理由不只是行数：**它是「工具表长什么样」这一件事，而 `main`
的职责是把各模块串成一次启动**——两个层面混在一个函数里，读 main 的人得先跳过三十
行工具表细节才能看到下一步。红线 11「顺序是契约」的那段解释也跟着搬进新文件的模块
文档，离它约束的代码更近。

结果：`main.rs` **291 行**，`tool_table.rs` 79 行，`agent_limits.rs` 188 行。

## 坑：`rustfmt` 收 crate root 会递归格式化整个 crate

为避开另一会话在飞的 `agent-wasm`/`agent-transport`，本次刻意不跑 `cargo fmt`，改成
「`git status` 取自己动过的文件 → 逐个 `rustfmt`」。**但清单里有 `lib.rs`/`main.rs`
——它们是 crate root，`rustfmt` 会顺着 `mod` 声明递归格式化整个 crate。** 结果 62 个
本次根本没碰过的文件被重排（`agent-cli` 三个 + `agent-runtime/tests/it/` 五十多个），
其中不少是本仓当前状态就不是这一版 rustfmt 干净的老文件（例如 `session_start.rs` 的
`{run_session_start, ToolTable}` → `{ToolTable, run_session_start}`）。

已按「diff 里不含 `AgentLimits`/`limits`/`tool_table::` 即无关」筛出来 `git checkout`
恢复，只留真正涉及的 29 个改动文件。**下次要点**：给 `rustfmt` 传 crate root 等于
格式化全 crate；只想格式化个别文件就别把 `lib.rs`/`main.rs` 放进清单，或者事后按
上面这条判据筛一遍。
