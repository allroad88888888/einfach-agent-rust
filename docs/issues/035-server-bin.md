# 035 `agent-server-bin`：二十行宿主

**里程碑** M4 · **依赖** 无 · **模型** sonnet · **独立测试 agent** 否 · **状态** 完成

## 目标

决策 12 的另一半：库是主体，bin 只是众多宿主之一。企业不用它也能内嵌，用它就是
开箱即跑。

## 做什么

`crates/agent-server-bin`（workspace member）：`main.rs` 目标二十行量级——读
providers.toml（路径可 `--config` 覆盖）、会话目录 `--sessions-dir`（每会话一个
jsonl，POST /sessions 的 session_path 语义对齐）、端口 `--port`/`AGENT_SERVER_PORT`、
红线 8（bind 默认 loopback，`AGENT_BIND` 显式覆盖）、Ctrl-C 优雅退出（close 全部
会话落快照）。日志就是 stdout 几行（决策 11：不集成日志规范）。

## 验收

起进程 → curl 建会话/对话（假上游不可行则真上游一轮，主会话跑）→ Ctrl-C →
会话文件完整可恢复。`--help` 可读。红线检查过（尤其 8）。

## 注意

serve.rs example 保留（联调用途不同）；bin 里重复的装配逻辑若超过三十行，
提库函数（`agent_server::bootstrap` 之类），example 一起换用。

## 实做记录（实现 agent，2026-08-02）

**落地**：新 workspace member `crates/agent-server-bin`（二进制名字仍叫
`agent-server`，跟 crate 目录名分开——运维看 `ps`/日志认前者）。`main.rs`
19 行（含 `mod` 声明与文档注释在内 19 行物理行，逻辑部分只有 6 行）：解析
参数、按 `Help`/`Run` 分支调库，装配细节全部不在这个文件里。

### 库这一侧：`agent_server::bootstrap` + `SessionsHandle`（issue「注意」条目落地）

`examples/serve.rs` 原本手写的「读 providers.toml → 选 provider → 查 key →
拼 `SessionTemplate`」这段收进新模块 `crates/agent-server/src/bootstrap.rs`
（`BootstrapOptions`/`Bootstrapped`/`BootstrapError`/`bootstrap()`，124 行）。
`examples/serve.rs` 换用它（80 行，比原来短，且跟 bin 共享同一份配置加载/
错误文案）——issue「注意」条目「example 一起换用」按字面做到。036 的桌面
内嵌预期也会调这个函数（`tools_root`/`default_sessions_dir` 走平台标准目录，
其余不变），接口文档写清楚了「管什么/不管什么」的边界。

### `--sessions-dir` 语义：查了 `SessionRegistry::open` 现状，确实缺自动分配

按要求查过——`POST /sessions` 不带 `session_path` 此前只有一条路：
`OpenSpec.store_path = None` → `Memory`，没有「不带路径也落盘」这个选项。
最小补法：`SessionTemplate` 加一个新字段 `default_sessions_dir: Option
<PathBuf>`（`crates/agent-server/src/http/config.rs`），`open_spec` 在没收到
显式 `session_path` 时，如果这个字段是 `Some(dir)`，自动分配
`dir/<session-id>.jsonl` 并现 `create_dir_all(dir)`——不预先建目录会踩一个真
坑：`agent_runtime::jsonl::Jsonl` 的 IO 线程在目录不存在时打不开文件，只会
静默报一次 `on_error`（`TransportTrouble` 事件），`open()` 本身仍然「成功」，
session 表面正常、实际啥也没落盘，没人会主动发现。三个单元测试钉住三种组合
（无 default 无显式 = Memory；两者都给 = 显式赢；只给 default = 自动分配且
目录被现造）。

### Ctrl-C 优雅关：库加了 `AgentServer::sessions()`/`SessionsHandle`

优雅关闭需要「枚举当前打开的 session、逐个 close（等 actor 线程处理完手头
的活再 join，保证落盘完整）」，但这个能力原来完全在 axum 路由内部（`AppState`
包着 `SessionRegistry`），bin 拿不到。最小加法：
- `SessionRegistry::ids()`——表里全部 id（不分死活）。
- 新文件 `crates/agent-server/src/http/sessions_handle.rs`：`SessionsHandle`
  （`AppState` 的一层克隆），只裁「关」这一半（`ids()`/`close_all()`），不给
  「开」的能力（`open()` 仍然只在 `POST /sessions` 路由内部可达）。
- `AgentServer`/`BoundAgentServer` 各加一个 `&self` 方法 `sessions()`——在
  `bind`/`serve` 消费掉 `self` 之前（或之后，两个都提供）先借出这份把手，
  两者背后是同一份 registry。

`agent-server-bin` 的用法：`tokio::select!` 里一路是 `bound.serve()`，另一路
是 `tokio::signal::ctrl_c()`；后者触发时把 `sessions.close_all()`（阻塞，内部
`join` 每条 actor 线程）扔进 `tokio::task::spawn_blocking`，等它返回、打印每个
session 的关闭结果，再让 `run()` 自然返回——进程以退出码 0 正常结束，不是
`process::exit` 硬切。

### 依赖选择

命令行解析：手写（`crates/agent-server-bin/src/cli.rs`），照抄 `agent_cli::
session_path::resolve` 的「遍历 args，认 `--flag value`/`--flag=value`，`--help`
随时短路」手法，没引入 clap——三个 flag 换不回子命令/自动补全的收益。Ctrl-C：
`tokio::signal::ctrl_c()`（`agent-server-bin` 的 tokio 只开 `rt-multi-thread`/
`macros`/`signal` 三个 feature），不是 `agent-cli` 那条 `ctrlc` 依赖的路——
`agent-server-bin` 本来就是 `#[tokio::main]`，tokio 自带的信号处理比再引入一个
sync 风格的 crate 更省。

### 触碰到的既有文件（全部加法/最小改动）

- `crates/agent-server/src/http/config.rs`：`SessionTemplate` 加一个字段、
  `open_spec` 加一段分支、三个新单元测试。
- `crates/agent-server/src/http/mod.rs`：`AgentServer`/`BoundAgentServer` 各
  加一个 `state: AppState` 字段和一个 `sessions()` 方法；模块文档表格加一行。
- `crates/agent-server/src/registry/mod.rs`：加 `ids()` 方法 + 一个单元测试。
- `crates/agent-server/src/lib.rs`：加 `mod bootstrap` 及对应 `pub use`，
  `SessionsHandle` 并进既有的 `pub use http::{...}`。
- 两个既有测试夹具（`tests/support/http_server.rs`、`tests/http_indep_support/
  server_harness.rs`）因为 `SessionTemplate` 加字段各补一行
  `default_sessions_dir: None`。
- 根 `Cargo.toml`：workspace members 加 `crates/agent-server-bin`。

036/037 在并行改的文件（`ServerConfig::with_static_dir`/`static_files.rs`、
`apps/`、`examples/java-gateway/`）没有碰；`http/mod.rs`/`Cargo.toml` 这两个
036 也在改的文件，只做了针对本 issue 的定点加法（新增字段/方法/依赖行），
没有改动 036 已经写好的 `static_dir`/`tower-http` 那部分。

### 收工验证（前台，原文见下）

- `cargo run -p agent-server-bin -- --help`：可读，见上方粘贴的完整输出。
- 起进程（`--sessions-dir <dir> --port 0`）→ `curl -X POST /sessions`
  （空 body，零真实上游调用，没发任何 `/input`）→ 201，`<dir>/<id>.jsonl`
  自动出现（0 字节，符合「open 了但没有任何轮次」的预期）→ `GET /sessions/:id`
  200 alive → `kill -INT` → stdout 打出「Ctrl-C：优雅关闭全部会话…已关闭 1
  个会话，退出。」→ 进程干净退出（非 `kill -9`）→ 第二个进程用同一个文件路径
  当 `session_path` 重新 `POST /sessions` 成功 201（证明文件是完整、可恢复的
  jsonl，不是半截坏文件）→ 再次 Ctrl-C 干净退出。
- `cargo test --workspace`：**954/0**（含新增的 agent-server 6 个单测 +
  3 个集成测试文件 + agent-server-bin 5 个单测），零 FAILED。
- `cargo clippy --workspace --all-targets -- -D warnings`：零警告。
- `scripts/check-invariants.sh --all`：红线检查通过。
- `wc -l`：新增/改动文件全部 ≤300（最长 `http/config.rs` 214 行、
  `registry/mod.rs` 253 行；`main.rs` 19 行，`run.rs` 101 行，`cli.rs` 159 行，
  `bootstrap.rs` 124 行，`sessions_handle.rs` 55 行）。

### 异议 / 未做的事

- **`--sessions-dir` 只做了「不带 `session_path` 时自动分配」，没有校验
  `session_path` 落在 `--sessions-dir` 之内**——issue 原文与 031 的既有语义
  都允许客户端在 `POST /sessions` 里给任意 `session_path`（`Jsonl` 早就是
  这个行为），`--sessions-dir` 是「默认值」不是「白名单/沙箱边界」，没有把
  它做成后者——如果这是期望的安全语义，需要单独立一条 issue（这属于「校验
  客户端可以把 session 落到文件系统任意位置」这个更大的问题，031 就已经
  存在，不是本 issue 引入的新缺口，035 的范围是「不给路径时怎么办」）。
- **`--sessions-dir` 的自动分配没有清理机制**——close 之后文件留在磁盘上
  （这本来就是持久化的意义），但重开同一个进程会不断新建 `sess-<pid>-<n>.jsonl`，
  没有任何过期/清理策略。ROADMAP 没有点名这属于 M4 范围，如实留白。
- **没有做「假上游」版本的一轮真实对话冒烟**——上级任务明确要求「零真实上游
  调用（input 不发）」，验收里「假上游不可行则真上游一轮」这个分支没有触发；
  真实的「一问一答」轮次仍然是通过库层既有测试
  （`tests/http_post_input_streams_deltas_then_terminal.rs` 等）在假上游上
  覆盖的，不是这次新写的。
- **`SessionsHandle::close_all` 的并发关闭是串行的**（`ids().into_iter().map`
  逐个调用同步 `close`，不是并发关）——M3 单副本场景下会话数量不大，串行关闭
  的总耗时不构成问题；真要并发关（比如上百个会话）需要另起讨论，没有在这次
  加过度设计。

### 合并记录（主会话代收尾）

装配收编成 agent_server::bootstrap（serve.rs example 换用），bin 三文件
（cli/main/run）。主会话冒烟：建会话自动落盘 <dir>/<id>.jsonl、SIGINT 优雅
关闭报数、--help 自文档。实现 agent 收尾自旋（第五例）——此模式病记入
M4 收官清单：WORKFLOW 收工模板补硬性防自旋条款。
### 补记（agent 迟到的完整报告，公道起见）

自旋数轮后最终交卷，且有增量真货：①发现并修 Jsonl 对缺失目录静默失败的暗雷
（会话看着开了、盘上零字节都不落——create_dir_all 前置 + 3 单测）；②冒烟推进到
「Ctrl-C 后重开同一文件」证明可恢复非截断；③三条如实异议（sessions-dir 是默认
不是沙箱、无保留策略、close_all 串行）。workspace 954/0。自旋病结论不变：
活干得好，收尾模式坏——两件事分开记。