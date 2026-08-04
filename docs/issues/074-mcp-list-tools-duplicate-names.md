# 074 同一个 MCP server 返回两个同名工具，整条链没人拦

**里程碑** M10 期间捞到 · **依赖** — · **模型** sonnet · **独测** ✅（碰红线 11 + 可逆性静默错档）

069 拍板撞名策略时，按「一个名字在进 prompt 的表里只能出现一次」这条红线复查四条来源，
在 `agent-mcp` 上捞到的。**不是设计问题，是一条运行时够得着的实现缺口。**

## 现象

`crates/agent-mcp/src/client.rs:118` 的 `list_tools`：

```rust
let tools = parse_tools_list(&result).map_err(McpError::Protocol)?;
Ok(tools.iter().map(|t| translate(t, server_id)).collect())
```

`parse_tools_list`（`protocol.rs:82`）也只是 `tools.iter().enumerate().map(parse_one_tool).collect()`
——**整条链没有任何一处判重**。一个 server 的 `tools/list` 回包里有两项 `name` 相同，
就翻译出两条 `mcp:<server>/<tool>`，一路进 `ToolTable::with_mcp`。

## 为什么这条不是小瑕疵

进表之后**两个后果不对称，坏的是第二个**：

1. **模型看到两份说明书**：`specs` 是 `Vec`，`push` 两次，两条同名 spec 都进 prompt。
   红线 11 的钱白花，模型还得在两份 schema 之间猜。这条至少**看得见**。
2. **undo 屏障按错的那份办**（真正贵的）：`mcp_reversibility` 是 `BTreeMap`，
   `insert` 同一个 key 两次是**后来居上**。于是 server 只要把两条同名工具的
   `readOnlyHint` 写得不一样（一条 `true` → `Pure`，一条没写 → 保守档），
   **模型调的是第一份的说明书，屏障用的是第二份的可逆性**。
   功能完全正常、不报错，只有 `/undo` 撞上它的时候才以错值浮出来——
   红线 6/11 那一类「静默错值」的典型形态。

**工具表内部今天就不自洽**：同一批 `(spec, reversibility)`，spec 走 `Vec::push`
（两份都留）、可逆性走 `BTreeMap::insert`（只留最后一份）。两种容器两种撞名语义，
谁都没错，合起来就是上面第 2 条。069 已把「表侧统一成整条丢弃」列进它的代码清单
（等 062 之后做，动 `tool_table.rs`）；**本 issue 管的是更靠前的那一跳**。

## 拦在哪：`list_tools`，不是表

按 069 拍板的判据——**在最早能报给「有权修它的人」的那个点上失败**：

- 能修这个的人是**配置了这个 MCP server 的部署方**，不是写 agent 的人、更不是模型。
- 他们能被告知的时机是**连接/装载那一刻**，不是第 40 轮对话中途。
- `list_tools` 正是那一刻，也是链上最早知道「这一批里有重名」的地方。

表侧的整条丢弃是**兜底**（防的是别的来源），不是这条的解法：等到了表里，
「哪个 server 给的」这个信息已经只剩名字前缀，报出来的话也没法指导人去改哪个配置。

## 范围

1. `list_tools` 在翻译前后做**同 server 内**去重：**保留第一条，丢弃后来的整条**
   （与 069 给工具表定的方向一致：只加不改，丢后来的）。
2. **必须留下能到达部署方的痕迹**——不许静默丢。走本 crate 既有的告警/日志路子
   （先去看 `McpRegistry` 连接失败、握手降级现在是怎么报的，**照那条路走，别新发明一套**）。
   报文里要有：server id、重复的工具名、丢了几条。
3. **不做**跨 server 去重——`mcp:<a>/x` 和 `mcp:<b>/x` 名字本来就不同，069 把
   「MCP 靠命名自带 server id」列为四条路里的**范本**，别把它改坏。
4. **不改** `ToolTable`（069 的清单里那两条要等 062 之后、且会顶破 300 行要拆，
   另有其人）。本 issue 只碰 `crates/agent-mcp/`。

## 验收（可判定）

- **去重生效**：造一个 `tools/list` 回包，同 server 内两项同名（**两项的
  `annotations.readOnlyHint` 故意写得不一样**）→ `list_tools` 只吐一条，且是**第一条**，
  可逆性是第一条的那一档。
- **告警真的发出去了**：断言那条报文存在，且**含 server id 与重复的工具名**
  （只断言「有告警」不够——没有 server id 的告警对部署方等于没有）。
- **不误伤**：同一批里名字不同的照常全过；空数组、单条照常。
- **跨 server 同名不受影响**：两个 server 各有一个 `x` → 两条都在，名字分别是
  `mcp:a/x`、`mcp:b/x`。
- **突变验证（必须做，贴真实红/绿输出）**：把去重那行删掉 → 第一条与第二条断言变红 →
  改回 → 绿。069 的先例：本仓有过「构造对突变免疫」的白写测试，
  **没红过的护栏不算护栏**。

## 注意

- 红线 11 相关，但**本 issue 不进 prompt 那一层**——你只保证吐出去的 `Vec` 里名字唯一，
  排序/字节确定性归 063。
- 红线 9：≤300 行。
- **别顺手改 `translate`/`parse_one_tool` 的语义**（041 的既有形状，有测试钉着）。
- 收工验证前台跑完（WORKFLOW §四 -1），含 `cargo test -p agent-mcp`、
  `cargo clippy -p agent-mcp --all-targets -- -D warnings`、`check-invariants.sh --all`。

## 实做记录（完成 · 2026-08-04）

**去重放在 `McpClient::list_tools` 这一跳**（`crates/agent-mcp/src/client.rs`），新增私有
`dedup_by_name(tools: &[McpTool], server_id: &str)`：按 `McpTool.name` 判重，`kept_names`
只在没见过这个名字时才登记，撞上的那条既不进 `kept` 也不重新 `translate`——**保留第一条、
后来的整条丢弃**，`translate`/`parse_one_tool` 一行没动。

### 告警走的哪条既有路

先看过这个 crate 现在怎么报「连接失败」：`loader::connect_stdio` 把连接/`tools/list` 失败
翻成 `Availability::Unavailable{reason: String}`——**结构化状态，不是日志**，是这个 crate
唯一的「报给部署方」的路子（`docs/MCP.md`/`status.rs` 里没有 `tracing`/`log`/`eprintln!`
这类东西，grep 过一遍确认）。但重复工具名不该让整个 server 变 `Unavailable`（server 照常
连上，只是丢了几条工具），而 `Availability::Connected{tool_count}`/`ServerStatus` 又在
`agent-cli::print::mcp` 被**穷尽 match**（无 `..`），加字段/加变体都会让那边编译不过——
本 issue 硬边界不许碰 `agent-cli`。所以选了同精神、不破坏兼容的落点：

1. `client.rs` 新增 `pub struct DuplicateToolWarning { server_id, tool_name, dropped }` +
   `Display`（跟 `McpError`/`ProtocolError`/`ConfigError` 同一种「结构化 + Display」形状，
   不是拍脑袋发明的）；`list_tools` 返回类型从 `Vec<(ToolSpec, Reversibility)>` 改成
   `ToolListOutcome = (Vec<(ToolSpec, Reversibility)>, Vec<DuplicateToolWarning>)`
   （起了个类型别名单纯是绕开 clippy `type_complexity`）。
2. `loader.rs` 的 `LoadOutcome` 加一个新字段 `pub warnings: Vec<String>`（`connect_stdio`
   把 `DuplicateToolWarning` `.to_string()` 之后塞进去）——**只加字段，不改已有字段**，
   `agent-cli::mcp::bootstrap` 只按字段名取 `outcome.tools`/`outcome.servers`，没有穷尽
   解构，编译验证过不受影响（见下）。`/mcp` 命令要不要把 `warnings` 展示出来是
   `agent-cli` 的事，本 issue 硬边界不许碰，先把痕迹送到边界上。

报文格式：`` MCP server `<id>`: tools/list 里工具名 `<name>` 重复，丢弃了 <n> 条（保留第一条） ``
——server id、重复的工具名、丢了几条三样都在，不是只断言「有告警」就算数。

### 没有碰、但验证过不会碰坏的东西

`Availability`/`ServerStatus`/`LoadOutcome.tools`/`LoadOutcome.servers` 的既有形状一个字节
没变；`cargo build -p agent-cli -p agent-runtime --tests` 全绿（在改动落地后跑过），
证明加字段是真的兼容，不是猜的。

### 突变验证（真实红/绿）

把 `dedup_by_name` 里的判重整段（`if kept_names.contains(...) { ...; continue; }`）删掉，
只留 `kept_names.push(&tool.name); kept.push(translate(tool, server_id));`，跑
`cargo test -p agent-mcp --test list_tools_duplicate_074`：

```
test same_name_three_times_drops_two_into_one_warning ... FAILED
test duplicate_tool_name_keeps_first_spec_and_first_reversibility ... FAILED

---- duplicate_tool_name_keeps_first_spec_and_first_reversibility ----
thread '...' panicked at crates/agent-mcp/tests/list_tools_duplicate_074.rs:52:5:
assertion `left == right` failed: 两条同名，只该留一条
  left: 2
 right: 1

---- same_name_three_times_drops_two_into_one_warning ----
thread '...' panicked at crates/agent-mcp/tests/list_tools_duplicate_074.rs:76:5:
assertion `left == right` failed
  left: 3
 right: 1

test result: FAILED. 2 passed; 2 failed; 0 ignored; 0 measured; 0 filtered out
```

（`distinct_names_all_kept_with_no_warnings`/`empty_tools_list_is_fine` 两条不涉及重复名字，
维持绿——这本身也是「不误伤」的一个交叉验证。）改坏的那段已经改回来，`grep -rn "MUTATION"
crates/` 干净。

### 「保留第一条」怎么被断言钉死

`duplicate_tool_name_keeps_first_spec_and_first_reversibility`（`tests/list_tools_duplicate_074.rs`）
构造两条同名 `echo`：第一条 `annotations.readOnlyHint: true`（→`Pure`）、第二条**没有
`annotations`**（→`Irreversible`）——两项故意写得不一样，断言 `tools[0].1 ==
Reversibility::Pure` 且 `tools[0].0.description == "first"`。如果去重改成后来居上（留最后
一条），这两条断言会红成 `Irreversible`/`"second"`，不会安静通过——「留哪条」因此是可判定的，
不是留着两条一模一样的摆设。

### 遇到的坑

- 第一版测试脚本用多行缩进的 raw string 拼 `tools/list` 的 `tools` 数组，`sh` 单引号里的
  换行原样传给 `printf`，把 newline-delimited JSON-RPC 的一帧切成了两半——`read_line` 在第
  一个内嵌换行处就停了，客户端拿到半截 JSON 报 `NotJson`。改成单行 JSON 拼字符串，并在
  `server_script` 里加一条 `assert!(!tools_json.contains('\n'), ...)` 防止再犯。
- `list_tools` 改签名后，两处既有调用点需要跟着改：`tests/handshake_translate_042.rs`
  （加 `warnings.is_empty()` 断言）、`tests/everything_server_042.rs`（真起
  `server-everything`，顺带断言真实 server 不产生重复工具名）。两个都在
  `crates/agent-mcp/` 范围内，不越界。
