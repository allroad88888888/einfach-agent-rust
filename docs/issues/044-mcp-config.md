# 044 `.mcp.json` 装载 + 多 server + 失败隔离

**里程碑** M6 · **依赖** 043 · **模型** sonnet · **独测** —

从「一个 server 硬编码起来」到「读配置、起多个、有的起不来也不拖垮会话」。解析错 / 连不上
当场知道，所以 sonnet。

## 范围

1. **`.mcp.json` 解析**：`mcpServers` 对象，跟 Claude Code 对齐。stdio 形状
   `{command, args, env}`。远端形状 `{type,url,headers}` **只解析、报「暂不支持」**，不实现
   （M6 范围外，但配置里出现不该让整个文件解析失败）。
2. **多 server 装载**：遍历 `mcpServers`，逐个 spawn + 握手 + `tools/list`，把所有工具汇进
   一张表，可逆性映射合并。server id = `mcp:<id>/` 命名前缀。
3. **失败隔离**：一个 server 命令不存在 / 握手失败 / 超时 → 标 `Unavailable`（带原因），
   **其余照常连、会话照常起**。不是「一个坏了全崩」。
4. **host 可用性门**：registry 表达「这个源在这个 host 上可用吗」。M6 的 CLI 是 server
   host，stdio 恒可用；这个门现在写死形状，为延后的 http（浏览器 host 只能 http）留位——
   不假装 stdio server 在浏览器存在然后调用才失败。
5. **server id 撞名** → 装载报错（不静默取后者）。

## 验收（可判定）

- 两个 server 的 `.mcp.json` → 两批工具都进表，各自 `mcp:a/` `mcp:b/` 前缀；两个 server
  各有同名 `x` → `mcp:a/x` 与 `mcp:b/x` 不撞。
- 一个 server 的 `command` 指向不存在的可执行文件 → 它标 `Unavailable`，另一个正常的 server
  工具照常在表里，`available_servers()` 能报出谁连上了谁没有 + 原因。
- 远端 `{type:"http",...}` 出现在配置里 → 解析不崩，该 server 标「暂不支持」，其余 stdio
  server 照常。
- server id 撞名（同一个 key 出现两次是 JSON 非法；两个 server 声明相同逻辑 id 的路径）→
  明确报错。
- host 门：`available_on(host)` 对 stdio + server host 返回可用；接口能表达不可用（http +
  浏览器 host 延后但形状在）。

## 注意

- **红线 8 邻近**：`.mcp.json` 里的 `command`/`args` 是要 spawn 的外部进程，装载路径别把它
  暴露成网络可控（配置来自本地文件，和 providers.toml 同信任级）。
- **失败隔离是产品判断**：Claude Code 的 `/mcp` 显示 failed server 不阻塞会话——照这个。
  错误进结构化状态（谁连上、谁没有、为什么），不是 panic、不是吞掉。
- **红线 3** 继续成立：多个 server 的活句柄都住 `McpRegistry`，配置（含可用性/原因）可序列化。
- 收工验证前台跑完（WORKFLOW §四 -1）。

## 实做记录（完成 · 2026-08-03）

全部落在 `agent-mcp`，按职责拆四个新文件（一个文件一件事，都 ≤300 行），CLI 接线留给 045。

### 建了什么

| 文件 | 行 | 干什么 |
|---|---|---|
| `agent-mcp/src/config.rs` | 275 | 新：`.mcp.json` → 结构化 `McpConfig`。**纯解析、零连接**。stdio `{command,args,env}`；远端 `{type,url,headers}` 只解析成 `Remote` 留位（不让整份失败）；撞名 → `ConfigError::DuplicateServerId`。撞名靠**流式 `visit_map` 保序保留重复 key**（不经 `serde_json::Value` 的 `Map`——那个静默取后者就查不出撞名了），在 Rust 层显式报 |
| `agent-mcp/src/availability.rs` | 66 | 新：host 可用性门 `Host` × `TransportKind`。`Host::supports`——server host stdio 恒可用（M6 CLI）；浏览器 host stdio 不可用（没有子进程）、只支持远端。**门表达能力，「实现了没」是 loader 的判断** |
| `agent-mcp/src/status.rs` | 69 | 新：装载后每个 server 的**可序列化状态** `ServerStatus{id, availability}`，`Availability` 三态：`Connected{tool_count}` / `Unavailable{reason}` / `Unsupported{reason}`。失败隔离把「起不来」变成一条结构化状态，不 panic、不吞（红线 3：配置与可用性进 store，活句柄在 store 外） |
| `agent-mcp/src/loader.rs` | 161 | 新：多 server 装载 + 失败隔离。`load_servers(config, registry, host, timeouts, name, ver) -> LoadOutcome`。遍历配置逐个 spawn+握手+`tools/list`，合并工具表（顺序=配置顺序，红线 11），活句柄进 `McpRegistry`。**永不整体失败**——单个 server 的问题落它自己的 `ServerStatus` |
| `agent-mcp/src/lib.rs` | +18 | 改：`mod`/`pub use` 四个新模块的公开面（`McpConfig`/`ServerConfig`/`Host`/`Availability`/`load_servers`/`LoadOutcome` 等） |
| 测试 | — | `tests/mcp_config_044.rs`（解析层，零 IO：两 server 保序 / 撞名报错 / http+sse 留位不崩 / host 门 `available_on`）、`tests/mcp_loader_044.rs`（装载层，`sh` 假 server + 不存在命令，零网络：两 server 同名 `x` 合并成 `mcp:a/x` vs `mcp:b/x` / 坏 server 隔离 / 远端标 unsupported / 浏览器 host stdio 不 spawn） |

### 配置数据形状

```
McpConfig { servers: Vec<(String, ServerConfig)> }        // 保序（红线 11）
ServerConfig = Stdio(StdioServer) | Remote(RemoteServer)  // Serialize/Deserialize（快照）
StdioServer  { command: String, args: Vec<String>, env: BTreeMap<String,String> }
RemoteServer { transport_type: String, url: String, headers: BTreeMap<String,String> }  // 留位
```

`env`/`headers` 用 `BTreeMap`（有序、可序列化，红线 11 不许 `HashMap`）。config 的
`Serialize`/`Deserialize` 是**内部快照格式**（externally-tagged 的 enum），跟 `.mcp.json`
的 wire 形状（`type` 判别、缺省当 stdio）不是一回事——wire 走 `RawServer`+`classify`，快照
走 derive，两个格式各管各的。

### host 可用性门（为延后 http 留位）

`ServerConfig::available_on(host)` = `host.supports(self.transport_kind())`。穷举：

| | server host | 浏览器 host |
|---|---|---|
| stdio | ✅ 可用（M6 CLI 走这条） | ❌ 不可用（没有子进程，接口能表达） |
| 远端 | ✅（能发 http，但 M6 未实现→loader 标 unsupported） | ✅（浏览器只能 http，延后但形状在） |

「host 支持」≠「M6 已实现」：远端在 server host 上 `available_on` 返回 true，但 loader 单独
判「M6 未实现 http 传输」标 `Unsupported`——两件事分开，等 http 传输的延后 issue 来了门不用改
破坏性接口。

### 失败隔离怎么表示成可序列化状态

`load_servers` 返回 `LoadOutcome { tools, servers: Vec<ServerStatus> }`，`servers` 就是
「谁连上了、谁没有、为什么」的可序列化报告（`available_servers()` 即返回它）。单个 server 走
`load_one`：

- **远端** → `Unsupported{reason: "远端传输 http 在 M6 未实现…"}`，不进 registry。
- **host 门不通过**（浏览器+stdio）→ `Unavailable{reason: "stdio 传输在 browser host 上不可用"}`，**不 spawn**。
- **stdio 且门通过** → 真连：spawn+握手+`tools/list` 任一步失败 → `Unavailable{reason: "连接失败: …" / "tools/list 失败: …"}`（`client` 在函数返回时 drop，`StdioTransport::Drop` 杀子进程收尸，半连的 client 不塞 registry）；成功 → `Connected{tool_count}`，工具进合并表、活句柄 `registry.insert`。

其余 server 照常连、会话照常起——`load_servers` 对 `Vec` 里每个条目独立处理，一个坏了只影响
它自己那条状态。撞名在 `parse_config` 那层就拦了（`DuplicateServerId`），到 loader server id
已唯一，不会静默覆盖 registry。

### 收工验证（前台跑完，真实输出）

三道门禁前台各一条命令跑完，真实输出：

```
$ cargo test -p agent-mcp        # exit 0
test result: ok. 57 passed; 0 failed  （lib：含 config/availability/status 新单测）
     Running tests/everything_server_042.rs
test everything_server_handshake_and_tools_list ... ok   （真起 npx，缓存后 1.09s）
test result: ok. 1 passed; 0 failed
     Running tests/mcp_config_044.rs
test result: ok. 5 passed; 0 failed  （两 server 保序 / 撞名报错 / http+sse 留位 / host 门）
     Running tests/mcp_loader_044.rs
test result: ok. 4 passed; 0 failed  （同名 x 合并不撞 / 坏 server 隔离 / 远端 unsupported / 浏览器 stdio 不 spawn）
其余 041/042 遗留测试（handshake_translate/initialize/jsonrpc_*/tools_*/translate_*/
registry_not_in_snapshot）全部 ok，所有测试目标 0 failed

$ cargo clippy -p agent-mcp --all-targets -- -D warnings    # exit 0
    Finished `dev` profile（零警告）

$ bash scripts/check-invariants.sh --all
红线检查通过
```

跑完 `ps` 确认无遗留 `npx`/`server-everything`/`sh -c 'read` 进程（everything_server 的
`StdioTransport::Drop`、假 server 的 registry drop 都 kill+wait 收尸了）。

### 留给 045

CLI bootstrap 接线（读 `.mcp.json`、`load_servers` 装进 `ToolTable`+`RunnerCtx`、`--mcp-config`
覆盖）、`/mcp` 状态命令（列 `ServerStatus`）、kill-9 重启从配置重连——045 的范围，本 issue 不碰。
