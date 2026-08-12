# 161 `agent-server` 两个上限 flag

**里程碑** M18 · **依赖** [159](159-agent-limits-config-decision.md)（拍板选 A 才做）
· **模型** sonnet · **独测** ✅ · **状态** 完成（2026-08-12）

## 目标

`agent-server --max-agent-depth <n> --max-children <n>` 真的改到那两道闸，且**模型
看到的工具描述跟着改**。

## 现状

- `agent-server-bin/src/cli.rs`：**手写解析，不上 clap**（那个文件的模块文档记了
  理由）。已认五个 flag，形状是「`--flag value` 和 `--flag=value` 两种写法都收」。
  照抄，别引依赖。
- `agent-server-bin/src/run.rs:51`：`ToolTableSpec::Full { spawn_limits: AgentLimits::default() }`
  ——就是要接的那一处。
- 接下来全链是白拿的：`ToolTableSpec::spawn_limits()` 读口 + `actor/body.rs:86`
  的 `set_agent_limits` 已经把「工具描述那份」和「真正拦人那份」对齐做成一次函数
  调用（034 干的）。**不要再造第二条对齐路径。**

## 做什么

1. `cli.rs`：`Cli` 加 `max_agent_depth: Option<usize>` / `max_children: Option<usize>`，
   解析照既有五个 flag 的形状；`HELP` 常量补两行（含默认值 3 / 8）与 ENV 两行。
2. 环境变量兜底 `AGENT_MAX_AGENT_DEPTH` / `AGENT_MAX_CHILDREN`，命令行优先——
   跟 `--port`/`AGENT_SERVER_PORT` 同一个既有取舍。
3. `run.rs`：由这两个值构造 `AgentLimits`（没给的那一项取 `default()` 的对应字段，
   **不是整个结构体二选一**——只给 `--max-children` 时 depth 该留在 3）。
4. 同一个值传给 [160](160-recover-limits-param.md) 补出来的 `recover` 入参
   （`run.rs` → `bootstrap` → `SessionTemplate`，`actor/body.rs:66` 那处）。
5. `examples/serve.rs:53` 与 `registry/spec.rs:186` 保持 `default()` 不动——示例和
   模板默认档不该跟着长 flag。

## 验收

- 解析层（照 `cli.rs` 已有那套测试的形状，同一个文件里）：两种写法都认；只给一个
  时另一个仍是 `None`；不给两个都是 `None`；拼错的 flag 仍然 `Invalid` 且点名。
- **端到端那条（本 issue 的全部意义）**：`--max-children 2` 起 server → 建会话 →
  `GET` 到的 `srv:agent/spawn` 工具描述里的数字是 **2**，且 spawn 第 3 个子被
  `TooManyChildren { max: 2 }` 拒——**两个数字是同一个 2**。
- 只给 `--max-agent-depth 1`：children 仍是 8（部分覆盖不连坐）。
- 159 若拍了「下限钉 1」：`--max-children 0` 的行为按那条拍板断言（拒绝启动 or
  夹到 1，二选一，测试钉住选的那个）。

## 注意

- **红线 11**：工具表顺序一个字节不改——这条只改 spawn 那一项描述里的两个数字，
  不动表里任何位置。
- 解析失败的处置**跟随 159 产出第 3 条**，别自己现定；`cli.rs` 既有取向是「解析层
  不做验证性报错，交下游」（见那个文件的 `unparseable_port_is_silently_none_not_a_panic`
  测试及其注释）。
- Java 参考网关用 `ProcessBuilder` 拉起这个 bin（INTEGRATION.md）——**这条不改网关**，
  但要在 163 的文档清账里点一句「网关要非默认上限就往参数表里加这两个」。
- **`agent-server/src/actor/body.rs` 今天已经 320 行、红线 9 已在告警**。这条要动
  它的第 4 步（传 `recover` 入参）——只加一个实参的话不至于顶破更多，但**别顺手在
  那里长逻辑**；真加了就得拆（skill `one-file-one-thing`）。
- `cli.rs` 加两个 flag 会让它继续长（今天未超限，加完自己 `wc -l` 核一遍）。

## 实做记录（2026-08-12，完成）

**新模块 `agent-server-bin/src/agent_limits.rs`**（一个文件一件事：「两个上限从哪来、
怎么校验」）。`cli.rs` 只管命令行语法，值域判断和 env 兜底都在这里。

**两层拆分照 `agent_server::bind`**：`resolve(cli, depth_env, children_env)` 是纯函数
（不读环境变量），`from_cli_and_environment(cli)` 才读。理由就是 `bind.rs` 模块文档
那条——`std::env::set_var` 在 2024 edition 是 `unsafe fn`，而 `cargo test` 多线程
并发跑，测试里改进程级环境变量会串味。于是 env 路径的坏配置也能被并发安全地测到。

**取严的先例找对了**：开工时以为是「偏离 `--port` 的既有取向」，勘查后发现本仓**两种
取向都有先例**，判据是**有没有下游替它报错**——`--port` 有（`default_bind_addr`），
`AGENT_BIND` 没有所以硬失败（`BindConfigError` 的文档：「把打错的字符串当成没设，
是那种配置错了却看起来在正常运行的坑」）。上限属于后者，所以这不是偏离，是**跟随
更贴切的那个先例**。159 的拍板记录与决策 32 已按此更正。

**真机 smoke（不花钱那部分）**

| 命令 | 结果 |
|---|---|
| `--help` | 两个 flag + 两个 env 都在 |
| `--max-children abc` | 拒绝启动，退出码 2 |
| `--max-children 0` | 拒绝，文案指向 `disable_builtin: ["srv:agent/spawn"]` |
| `AGENT_MAX_CHILDREN=0` | 拒绝，错误同时点出 env 名与对应 flag，退出码 1 |

**退出码 2 vs 1 是既有分工**，不是不一致：命令行语法错走 `ParsedArgs::Invalid`（2，
跟 `unknown option` 同路），配置/启动失败走 `fail`（1，跟 `remote_tool_timeout` 同路）。
同一个逻辑错误从两个入口进来拿到不同退出码，记在这里备查——改它会破坏既有约定，
不值得。

**测试**：`agent_limits` 9 条（正整数/坏值/0 指路/默认档/部分覆盖不连坐/env 兜底/
命令行优先/env 坏值点名两种写法），`cli_tests` 6 条（两种写法/只给一个/**与 `--port`
取向不同的对照断言**/0 指路/缺值/`--help` 覆盖新 flag）。
另在 `agent-server/src/registry/spec.rs` 加 1 条**主验收**：配的上限真的走进模型看得见
的那份描述（`Full { spawn_limits: 2/3 }` → 描述里是 2 和 3，且不含默认档的「8 个」）。

**负向验证**：把 `spec.rs` 的 `.with_spawn(spawn_limits)` 临时改回 `.with_spawn(default())`
→ 那条主验收**确实变红**。这一半（配置 → 描述）断掉不会报错，模型只会看到 8、按 8
规划，然后撞上运维配的那道更紧的闸。

**拆分**（红线 9，本次改动的一部分）：`cli.rs` 加完两个 flag 到 332 行——正是本 issue
「注意」里预警的那条。测试段拆去 `cli_tests.rs`（`#[path]` 手法）→ **169 行**。
