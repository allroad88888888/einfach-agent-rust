# 042 stdio 传输 + 客户端握手

**里程碑** M6 · **依赖** 041 · **模型** sonnet · **独测** —

第一次让 `agent-mcp` 碰真东西：起一个**真子进程 MCP server**，走完握手，`tools/list`
拿到真工具翻译成 specs。022 之于 provider 的对应物——纯接线，连不上当场知道。

## 范围

在 `agent-mcp` 里加 stdio 传输 + 客户端生命周期：

1. **stdio 传输**：spawn 子进程（`command`/`args`/`env`），按 **newline-delimited JSON-RPC**
   读写（写 stdin、读 stdout 逐行）。stderr 单独收（server 的日志，不当协议帧）。
2. **握手**：`initialize`（带本仓 client 的 protocol 版本 + capabilities）→ 收 server
   capabilities → 发 `notifications/initialized` → `tools/list`。
   - **协议版本是协商，不是断言**（041 探针实测：真 everything server 回
     `"2025-11-25"`，本仓 `CLIENT_PROTOCOL_VERSION` 是 `"2025-06-18"`）。client 提议一个、
     server 在响应里回它将采用的那个——握手要**记下 server 回的版本并继续**，版本不等
     **不硬失败**（tools 的 list/call 形状在这几个版本间稳定，M6 只用 tools）。要么把常量
     提到 `"2025-11-25"` 跟当前目标 server 对齐，要么保持提议旧版靠协商兜——042 定。
3. **`McpClient`**：持有子进程句柄 + 一个「发 request 等 response」的同步方法（042 只需
   阻塞式请求-响应；异步在飞是 043 的宿主侧的事，client 本身给一个能被 IO 线程调用的
   `call(name, args) -> Result<...>`）。
4. **`McpRegistry`**：进程内、store 外的表，按 server id 存活的 `McpClient`（红线 3）。
   `atom` / 快照里**没有**任何这里的东西。

## 验收（可判定）

- **集成测试**（需 npx）：起 `npx -y @modelcontextprotocol/server-everything`，`initialize`
  成功，`tools/list` 返回 ≥1 工具，翻译出的 name 全是 `mcp:everything/<t>`。无网络 / npx
  缺失 → 测试 `skip` 并在输出说明（不静默假过）。
- server 提前退出 / stdout EOF / 握手超时 → `McpClient` 干净收尾返回 `Err`，**不 panic、
  不永久挂起**（红线：后台读子进程 stdin 的挂起模式，见全局规则——发请求要能超时放弃）。
- `McpRegistry` 里的句柄不可序列化本就编不进 atom；测试断言 registry 不在 `Session`/快照
  路径上（结构性 + 注释说明红线 3）。
- 关掉 client → 子进程被回收（无僵尸），reader 线程收敛。

## 注意

- **红线 3**：`Child`、stdin/stdout pipe、reader 线程句柄**住 `McpRegistry`**，atom 里只有
  server 配置（id/命令行/可用性）。这是本 issue 的头等约束——放错编译期挡不住，靠 review。
- **全局规则「后台跑 CLI 必须关 stdin」的镜像**：我们是 spawn 别人的 CLI，要保证 server 的
  stdin 我们控制（写完请求不要让它以为还有输入要读到永久挂起）；发请求-等响应要带超时。
- 参照 `agent-tools/src/exec.rs`/`shell.rs` 的既有子进程模式（`std::process` 的用法、
  超时、stderr 处理）——照抄那套，别新发明。
- 收工验证**前台跑完**（WORKFLOW §四 -1）：集成测试真起一次 everything server，前台看到
  绿，再交报告。不许「起后台 + 等通知」。

## 实做记录（已完成）

**加的文件**（全部在 `crates/agent-mcp/`）：

- `src/transport.rs`（267 行）：`StdioTransport` + `TransportError`。spawn 子进程
  （`command`/`args`/`env`，`env` 追加到继承的父环境之上，不清空——`npx` 要靠 `PATH`
  找 `node`）；一个常驻后台线程用 `BufReader::lines()` 读 stdout，逐行塞进
  `mpsc::channel`，`read_line(deadline)` 靠 `recv_timeout` 拿到「阻塞读但能被外部超时
  打断」的效果（std 没有自带「读一行但最多等 N 秒」的 API，这是 `agent-tools/src/
  shell.rs` 那套「后台线程 + recv_timeout」模式的变体：shell 是一次性等
  `wait_with_output`，这里子进程长驻要反复读多行，所以读线程常驻）；stderr 单独一个
  后台线程收进有界 `VecDeque`（最近 20 行，纯诊断，不当协议帧）。`Drop` 先
  `kill()` 后 `wait()`，保证子进程被真正收尸、不留僵尸。
- `src/client.rs`（200 行）：`McpClient` + `McpError`。`connect()` 走完握手
  （`initialize` → 解析 → `notifications/initialized`），**协议版本只记录 server 回的
  值，不比较是否等于 `CLIENT_PROTOCOL_VERSION`**（协商不是断言，见下面实测）。
  `list_tools()`/`call()` 都是阻塞式请求-响应，带超时。`await_response()` 是应答匹配
  的核心：有 `method` 字段的行（server 插播的通知）跳过继续等；`id` 不对号的响应防御性
  跳过；其余解析失败直接报 `Err`，不会因为一行垃圾傻等到超时。
- `src/registry.rs`（116 行）：`McpRegistry`——`Mutex<HashMap<String, McpClient>>`，
  `insert`/`remove`/`contains`/`server_ids`/`with_client`。**红线 3 的活句柄住这里**：
  `Child`/pipe/reader 线程全部在 `StdioTransport`（`McpClient` 内部字段），
  `McpRegistry`/`McpClient`/`StdioTransport` 都不 derive `Serialize`，也故意不 derive
  `Debug`（避免顺手打印/序列化的路子被开出来）。
- `tests/handshake_translate_042.rs`：握手协商版本、应答匹配跳过插播通知、
  `tools/list` 翻译、JSON-RPC error 对象、握手超时、垃圾响应——全部用 `sh` 假 server
  脚本，零网络依赖。
- `tests/everything_server_042.rs`：真集成测试（见下）。
- `tests/registry_not_in_snapshot_042.rs`：结构性证明红线 3——断言
  `agent-core/Cargo.toml`、`agent-store/Cargo.toml` 都不依赖 `agent-mcp`（类型层面
  够不着 `McpClient`/`McpRegistry`，不是约定是编译期不可达）。
- `Cargo.toml`：**无新增依赖**，042 只用 `std::process`/`std::io`（issue 原文「042
  只需阻塞式请求-响应」，用不上 tokio）。

**真 server 协商版本实测**：`npx -y @modelcontextprotocol/server-everything`
（package 版本 `2026.7.4`，本机 npm registry 是 `http://npmjs.deepfos.com/`）在
client 提议 `protocolVersion: "2025-06-18"` 时，**原样接受并回同一个版本**
`"2025-06-18"`——跟 041 探针记录的 `"2025-11-25"` 不同（server 包升级后行为变了：
只要 client 提议的版本在它支持范围内就直接接受，不是总回它自己的最新版本）。
这正好印证了「协议版本是协商不是断言」这条设计决策的必要性：042 的握手代码全程不
比较两个版本号是否相等，只记录 server 实际回的值。另外实测到 `tools/list` 的响应
之前会插播一条无 `id` 的 `notifications/tools/list_changed` 通知，`await_response`
的跳过逻辑就是照着这个真实行为写的。

**收工验证（前台跑完，真实输出摘要）**：

```
$ cargo test -p agent-mcp
running 46 tests（单元测试，含 transport/client/registry 内联测试）... ok
tests/everything_server_042.rs: 1 passed（真起 npx 走完握手 + tools/list）
tests/handshake_translate_042.rs: 6 passed
tests/registry_not_in_snapshot_042.rs: 2 passed
其余 041 遗留测试（initialize/jsonrpc_codec/jsonrpc_malformed/tools_call/
tools_list/translate_*）全部 ok
总计所有测试目标 0 failed

$ cargo clippy -p agent-mcp --all-targets -- -D warnings
Finished `dev` profile [unoptimized + debuginfo] target(s)（零警告）

$ bash scripts/check-invariants.sh --all
红线检查通过
```

集成测试真起了一次 `npx -y @modelcontextprotocol/server-everything`（本机 npx/网络
可达，两道预检——`npx --version`、`npm view <pkg> version`——都通过，没有触发
skip 分支），握手成功、`tools/list` 拿到 14 个真工具（`echo`/`get-annotated-message`/
`get-env`/`get-resource-links`/`get-resource-reference`/`get-structured-content`/
`get-sum`/`get-tiny-image`/`gzip-file-as-resource`/`toggle-simulated-logging`/
`toggle-subscriber-updates`/`trigger-long-running-operation`/
`simulate-research-query` 等），翻译出的名字全部是 `mcp:everything/<t>` 形状。
测试跑完后检查过 `ps aux`，没有遗留的 `npx`/`server-everything`/`sh -c` 进程——
`StdioTransport::Drop` 的 kill+wait 生效，reader 线程随 stdout EOF 自然收敛。

未覆盖/留给后续 issue：`tools/call` 的执行路由与在飞凭据（043）、`.mcp.json` 装载
与多 server 失败隔离（044）、CLI `/mcp` 状态与全链验收（045）。
