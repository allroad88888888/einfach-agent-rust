# 工具、Skills、MCP

## 模型看到的是一张扁平表

AI 只管挑名字。执行在哪、可不可逆，由 router 和 undo 逻辑各自读各自的字段——
**但这两个字段不在喂给模型的那张表上**。三层，别混：

```rust
// ① agent-core/src/value/tool.rs —— 喂给 provider 的静态声明，只有三个字段
pub struct ToolSpec {
    pub name: Arc<str>,          // "srv:fs/read" | "web:selection/read" | "read_file"
    pub description: Arc<str>,
    pub schema: Arc<serde_json::Value>,
}

// ② agent-core/src/value/tool.rs —— 一次具体调用的「发起时快照」
pub struct ToolCallRequest {
    pub tool: Arc<str>,
    pub input: Arc<serde_json::Value>,
    pub location: Location,           // Server | Web | Desktop —— router 派发
    pub reversibility: Reversibility, // Pure | Reversible | Irreversible —— undo / 崩溃恢复
}

// ③ agent-runtime/src/tool_table.rs —— 宿主侧的判定表，②由它产出
pub struct ToolTable { /* specs + skill registry + mcp:<server>/<tool> → Reversibility */ }
```

**位置与可逆性不是 spec 的字段，是宿主现算出来的。** `ToolTable::snapshot(name, input)`
造 `ToolCallRequest` 时：`location_of(name)` 按名字前缀推（外加一条白名单，见下），
`reversibility_of(name)` 按名字查一张硬编码的 `match`——**只有 `mcp:` 前缀走真正的查表**
（`mcp_reversibility`，由 server 的 `readOnlyHint` 翻译而来，查不到落保守 `Irreversible`）。
「每个工具带着自己的元数据」是设计方向，今天只有 MCP 那一路真的是元数据，其余是名字规则。

M10 的宿主声明（`capabilities.tools[].reversibility`）是第二个真元数据入口，协议层已经收
（[061](issues/061-capabilities-protocol.md)），但装配还没接上——`ToolTable::snapshot` 的
查表分支门今天仍然只认 `mcp:` 前缀，宿主声明的等级会被 `reversibility_of` 的兜底盖成
`Irreversible`。**朝安全方向失效**（只会多问一次），但 062 落装配时必须一并拓宽那个门。

**没有 `Source` 枚举**（`Builtin | Mcp(ServerId) | Skill(SkillId)`）。工具从哪来今天只体现在
「谁把它 `push` 进这张表」——`builtin()` / `with_shell()` / `with_skills()` / `with_spawn()` /
`with_status()` / `with_collect()` / `with_mcp()` 各是一档独立授权，表里不留来源。
`capabilities` 校验里的 `Origin` 只用于错误文案，不回填工具表。要加 `Source` 之前先答
「谁读它」：今天没有读者，加了就是第二份要维护的真相。

叫 `Reversibility` 不叫 `Effect`——`Effect` 留给 loop 的「该发生什么」
（[issue 001](issues/001-loop-contract.md)），撞名是上一版真踩过的坑。

`location` 和 `reversibility` 是**正交的两个维度**。一个前端 tool 可以是不可逆的
（`web:clipboard/write`），一个服务端 tool 可以是纯的（`srv:fs/read`）。别合并。

### 命名空间：两族并存

约定是 `<location-prefix>:<namespace>/<tool>`，MCP 再多一层 server id：`mcp:<server>/<tool>`。
`location_of` 按前缀推位置：`web:` → `Web`、`desk:` → `Desktop`、`mcp:` → `Server`
（MCP 调用在宿主本地起子进程往返，不需要远端回传）、其余落 `Server`。

**但只有一族工具遵守这个约定**：

- **遵守**：`srv:fs/read`、`srv:shell/exec`、`srv:skill/activate`、`srv:agent/spawn` 等
  由 `ToolTable::builtin()`/`with_*()` 装的；MCP 翻译出来的 `mcp:<server>/<tool>`；
  M10 宿主注入的（前缀由校验**强制**，见下）。
- **不遵守**：`ToolTable::standard()` / `standard_local()` 那一族——`read_file`、`list_files`、
  `search_files`、`rg_search`、`apply_patch`、`write_file`、`delete_path`、`copy_path`、
  `move_path`、`revert_workspace_change`、`find_test_lint_commands`、`git_diff_review`、
  `run_task`、`run_verification_command`、`shell_macos`/`shell_linux`/`shell_powershell`
  ——**全是裸名**。这是刻意的：这族名字要跟既有 web-agent 的工具名逐字一致，改名等于改
  模型侧契约，而且工具表在 prompt 最前面，改一个字就是一次全量前缀失效（红线 11）。

裸名一族里有三个必须跑在**前端**：`ask_user_question` / `browser_action` / `save_file`。
它们没有 `web:` 前缀，于是 `location_of` 开头有一条**硬编码白名单**把它们捞成 `Location::Web`：

```rust
fn location_of(tool: &str) -> Location {
    if matches!(tool, "ask_user_question" | "browser_action" | "save_file") {
        return Location::Web;
    }
    // …按前缀推，兜底 Location::Server
}
```

**这条例外的代价要知道**：兜底分支是 `_ => Location::Server`。再加第四个前端工具时，
若既没写 `web:` 前缀、又忘了加进白名单，它会被静默判成本地执行 → dispatch 送进本地
executor → 模型只收到 `unknown_tool`。**不 panic、不告警，只是这个工具永远调不通。**
而白名单（`agent-runtime/src/tool_table.rs`）与那三个声明
（`agent-tools/src/interaction_specs.rs`）之间**没有任何编译期或测试期的绑定**——
加前端工具必须同时改两处。想消掉这条隐式耦合，就让 `location_of` 从 `interaction_specs()`
反查，而不是再记一个名字。

M10 注入的工具没有这个坑：`capabilities` 校验**强制**工具名以 `web:`/`desk:` 开头，
`srv:`/`mcp:`/裸名一律 400。理由是同一条——位置从前缀推，注入的工具跑在宿主侧，
标成 `srv:` 会让 dispatch 去本进程里找一个根本不存在的实现。

### 撞名

企业级多来源一定会撞名——两个 MCP server 各有一个 `search`，前端和后端各有一个 `read_file`。
[issue 069](issues/069-name-collision-policy.md) 拍了板：**不统一成一条行为，统一成一条
红线加一条判据。**

**红线（唯一一条，四条路都得满足）：一个名字在进 prompt 的那张表里只能出现一次，
且它的描述/schema 必须就是 dispatch 真会执行的那一份。**
`declares()` / `snapshot()` / 五条截获闸 / 远端第五路**全部按名字查**——一个名字只有
**一条**执行路径。两条同名 spec 一起进 prompt 不是「让模型来选」，是给模型看两份说明书
而只有一份对得上真正会跑的那件事；模型按哪一份的 schema 出参完全看它自己，以及 provider
怎么处理重复的 function 声明（三家都没探过）。**这正是本仓最怕的那类静默错值，只不过
发生在 prompt 里。**

**判据（决定每条路怎么满足红线）：撞名一律在「最早能报给有权修它的人」的那个点上失败；
那个点不存在时，才退到「后来的整条不进表」，绝不退到「两条都进表」。**
四条路行为不同，是因为「谁写的、还能不能被告知」不同：

| 路径 | 撞名的作者 | 最早可报点 | 行为 |
|---|---|---|---|
| **M10 宿主声明**（`capabilities`） | 客户端，在一次**活的请求**里 | **就是那次请求** | **整份 400**，会话不创建 |
| **MCP 多 server** | 第三方 server | 配置解析 / 握手 | 重复 server id 是**硬错误**（`ConfigError::DuplicateServerId`）；工具名自带 server id，跨 server **结构上撞不了** |
| **skill 目录装载**（`SkillRegistry::load`） | 部署者，而且**目录顺序是他显式排的** | 没有——先后本身就是他给的信息 | **后来居上**（有意的例外，见下） |
| **内置工具表装配**（`ToolTable::with_*`） | 一半是程序员（五档 + CLI 链），一半是运行时数据（MCP 回包、客户端请求体） | 程序员那半 = **CI** | **`debug_assert!` + 看门狗测试**；运行时那半 = **后来的整条不进表**，不 panic |

**为什么 `capabilities` 是「拒绝」**：两个候选出自同一份声明、同一个作者、同一口气，
没有先后可言，替它选一个就是替它做决定，而它此刻还站在那儿等 200/400——
**宿主自己都没想清楚要哪个，server 替它选一个只会把问题推到运行时**
（`agent-server/src/http/capabilities/validate.rs` 模块文档；同一条理由也是「绝不
sanitize 名字」的理由：悄悄把 `web:a b` 洗成 `web:a_b`，两个本来不同的声明就撞成同一个）。
**被否的是「后来居上」**：数组顺序是客户端序列化出来的，同一份声明两次连接可能不同序，
拿它当仲裁依据等于掷骰子。

**为什么 skill 目录是「后来居上」而不跟着拒绝**：目录顺序是部署者**显式给的**
（内置 → 项目 → 用户），「后面盖前面」正是覆盖机制本身的用途——用户想改写内置 skill 的
写法，那是产品需求。它也**不违反红线**：`BTreeMap::insert` 是**整体替换**不是字段级
merge，合并完每个 id 恰好一份，撞名在进 prompt 之前就没了。改成拒绝的代价是：一个用户
往 `~/.../skills/` 里放个跟内置同名的目录，进程就起不来——把一次本该生效的覆盖变成一次
启动失败，方向反了。

**MCP 那两条是范本**：它在**名字层面**消歧（`mcp:<server>/<tool>`），比任何冲突策略都早
一步——撞名压根不会发生，于是不需要仲裁。**统一的方向就是这个**：能靠命名让撞名不可能
发生的，就不要去写策略。宿主注入那一路已经在吃这个红利——061 强制 `web:`/`desk:` 前缀，
而**内置五档一个都不用这两个前缀**，所以注入的名字结构上撞不上内置的名字。
这才是「061 只在一份声明内部判唯一」够用的真正依据，不是运气；它由
`agent-runtime/tests/tool_table_names_are_unique.rs` 第二条断言钉住，破了会先红。

**工具表这一条是唯一今天还违红线的**（069 §拍板 D）：`with_*` 一路 `push` 不检测不去重，
而且表内部就不自洽——`with_mcp`/`with_host_tools` 往 `BTreeMap` 里 `insert` 可逆性是
**后来居上**，往 `Vec` 里 `push` spec 是**两份都留**：同一次撞名，prompt 看到两份、
可逆性只认最后一份。定下来的修法是「后来的那一条整条不进表」（spec 不 push、可逆性也不
insert），**不是 panic**——`with_mcp` 收的是第三方 server 的回包、`with_host_tools` 收的是
客户端请求体，让外部数据把宿主进程打死是把可用性交出去，而这两条路各自已经有更早的
裁判点。丢「后来的」而不是「先来的」是红线 11：工具表在 prompt 最前面，只加不改。
**实现排在 062 之后**（同一个文件正在被改），本次先落看门狗测试；已实测**当前五档 +
CLI 链没有任何撞名**，所以这个改动不会让既有装配组合变红。

**跨路径撞名（宿主注入的 `web:foo` × skill 激活时 `late_tools` 里的 `web:foo`）：表赢，
多余那份在 `skill_injection` 就滤掉。** 赢家不是选出来的——`declares()` 为真是因为**表**
里有它，远端第五路把调用派给宿主注册的那一份，skill 带的那份从来没有过自己的执行路径。
滤掉它执行侧一个字节不变，只是不再给模型看一份它影响不了的 schema。**这里绝不能报错**：
`skill_injection` 每轮都跑，作者早就不在场，轮中失败是「最早可报点」的反面。
**已落地**（064）：`agent-runtime/src/tool_table_skill.rs` 的 `skill_injection` 在返回前
`retain` 掉「表里已有的名字」；滤的是**工具**不是 skill——`late_system` 里那个 skill 的正文
一个字节不少。两条测试钉住（单测 `tool_table_skill_tests.rs` + 端到端
`tests/skill_late_tools_never_shadow_the_table.rs`，后者断在假上游收到的请求体上）。

（`late_tools` 完全不进 `declares()`，所以 skill 自带的 `web:`/`desk:` 工具今天执行不了
——那是**可执行性**的洞，不是撞名的洞；064 §范围 第 4 条明确「如实处理、别放大」，
本仓至今没有修它。）

## 位置透明路由

`agent-core` 只发 `ToolCall`，**不认识「前端 / 后端」这个概念**。router 看 `location`：

- `Server` —— 本地执行，await 结果
- `Web` / `Desktop` —— 往 SSE 上推 `tool_executing` 事件（载荷是 `{ call_id, request }`，
  `request` 就是那张发起时快照），把这一轮的工具槽置 `Pending`，等客户端
  POST `/tool_result` 回来结算

（事件名以 `packages/protocol/src/generated/SessionEvent.ts` 为准——它由 Rust 生成，
文档里不维护第二份清单。）

**对 core 而言两条路径完全同构**：发出去、置 `Pending`、等回写。这正是上游
`#BUSY!` 机制的现成落点，见 [STATE-MODEL.md](STATE-MODEL.md) §「Pending 的来历」。

所以 SSE 单向下行 + 普通 POST 上行就够了，不需要 WebSocket——服务端「反向调用客户端」
只是在流上推一个事件，客户端自己发一个请求回来。

### 回写必须匹配在飞的调用（epoch 由服务端保管）

```
POST /sessions/:id/tool_result   { agent, tool_call_id, result: { content, is_error } }
→ 202 Accepted
```

**客户端不带 epoch，也带不了。** 派发时服务端自己把当时的 epoch 记进
`RunnerCtx` 的等待槽（`PendingRemoteTool { agent, call_id, epoch, request, deadline }`）；
回写只需精确匹配仍在等待的 `(agent, call_id)`，匹配上了服务端才把当初那个 epoch 附回事件，
由 `Session::step` 的 epoch 闸校验（不等于当前世代就整条丢弃，一个 primitive 都不写）。

这不是「文档偷懒少写一个字段」，是**比让客户端报 epoch 更安全**：世代号伪造不了，
也没法拿一个猜出来的 `call_id` 去填别人的槽。红线 6 依然成立，只是校验点在
`RunnerCtx` + `Session::step`，不在请求体上。用户在结果回来之前按了 undo，
epoch 已经 bump，而且取消/undo/会话终止都会 `discard_remote_tools()` 清空等待槽——
迟到的回写连槽都找不到，走既有的拒绝路。

### reversibility 等级怎么定

这个字段决定 undo 能不能越过它，以及崩溃恢复时能不能重发。**定错了是数据事故**，
不是体验问题。

| 等级 | 判据 | 例 |
|---|---|---|
| `Pure` | 重复执行任意次，外部世界不变 | 读文件、查询、搜索 |
| `Reversible` | 有明确的补偿动作，且补偿本身可靠 | 创建资源（补偿=删除）、写入有版本的记录 |
| `Irreversible` | 其余全部 | 发邮件、支付、删数据、跑 shell |

**拿不准就是 `Irreversible`。** 判错成 Pure 的代价是重复发邮件；判错成 Irreversible
的代价只是多问用户一次。所以 `reversibility_of` 的兜底是 `_ => Irreversible`，
显式列出来的只有那几个已知纯读的名字（`srv:fs/read`、`read_file`、`rg_search`、
`srv:agent/status`、`srv:agent/collect`…）与两个有补偿动作的（`srv:agent/spawn` 的补偿是
`despawn_child`，skill 激活/停用的补偿是彼此）。

`undo` 往回走时撞上 `Irreversible` 的 entry → 停下，走 `UndoOutcome::Blocked`
（SSE 上是 `undo` 事件里嵌一个 `blocked`），让用户确认「继续（副作用不回滚）」还是取消。
落盘依据是 `EntryMeta.barrier`：宿主派发不可逆工具前调 `Session::mark_irreversible`，
随后那条 `tool_result` entry 就带上屏障位——**屏障是落盘的**，崩溃重启之后仍然拦得住。

## Skills

本质是「按需注入 context 的资产」——一段指令 + 若干文件，触发时进 prompt。

状态与内容分两边放：

```
skills_active (primitive atom, Slot::SkillsActive)   ← store 里只有「哪些被激活」
SkillRegistry (store 外)                             ← 正文与它自带的工具
```

换一个 skill 只重算注入料，不碰消息序列化。**这一条成立，但路径不是 derived atom**：
每一轮由 `ToolTable::skill_injection(active)` 现算出 `(late_system, late_tools)` 塞进
`Ingredients`，`prompt.system` / `prompt.payload` 这两个 derived 槽位至今没有落地
（见 [STATE-MODEL.md](STATE-MODEL.md) §「Derived atoms」）。结论不变，只是今天靠的是
「每轮现组」而不是「依赖图自动重算」。

Skill 携带的 tool 也是同一条路：激活时经 `skill_injection` 进这一轮的 `late_tools`，
停用时不再出现——**不是往工具表里增删**（工具表是会话期不可变的，红线 11：中途改它，
那一刻起前缀缓存全断）。`tools_registry_version` 那个槽位同理还没有写入点。

### 多来源与合并

`SkillRegistry::load(dirs)` 从若干**目录**装载（今天 `agent-cli` 只传一个
`<tool_root>/skills`；server 形态至今一次都没装载过 skill）。合并规则是**后来居上**——
069 拍板的**有意例外**，它跟工具表和 `capabilities` 不一样是有依据的，依据见上面 §撞名。

「内置 / 项目 / 用户 / 远端四个来源」是设计意图：目录多路已经支持，**「远端」这一路要等
M10 的宿主声明**（`capabilities.skills`，协议层已落地、装配在 064）。别为 skill 另造一套
解析规则——这条判断仍然作数；而「和 tool 同一套冲突策略」这句旧话**已作废**：069 定的是
一套共同的**判据**（撞名在最早能报给作者的那个点上失败），不是一套共同的**行为**。

**远端那一路合流时**（064）：server 形态**推荐不再从磁盘 `./skills/` 装载**——宿主已经有
声明入口，两个来源合流只会造出「同一份请求在不同部署上行为不同」的面。若仍要开，
宿主声明的 skill id 撞上磁盘已装载的 id → **400**（跟 061 同一条闸，那一刻客户端还在线，
符合判据）；**不许**静默让磁盘那份盖掉宿主声明的那份。

## MCP

**当成一个 adapter，不是核心抽象。** `agent-mcp` 的职责就是把 MCP server 暴露的 tools
翻译成本仓的 `(ToolSpec, Reversibility)`，经 `ToolTable::with_mcp` 喂进同一张表。

`resources` / `prompts` **目前不翻译**（握手时只留了 server 声明的 capabilities 原文）。
要接的时候它们该落成 skill 资产，那是接下来的事，不是已有的事。

### 服务端工具不是第四种 Location

有些 provider 能自己执行工具（检索、联网搜索），而**我们看不到**：响应里没有
`tool_calls`，也没有任何调用痕迹。它在模型内部发生，router 不参与也无从观测。

所以它不进 command log、undo 回滚不了、副作用等级无从判断、审计链路有洞。正确的建模
不是加一个 `Location` 变体，而是**一个会话级开关，开了就等于放弃这部分的可审计性**
—— 要显式承认并让用户知情，不是默认开着的便利功能。

（**这个开关还没有实现**，也还不必要：三家 adapter 的 `encode` 今天都只发
`Ingredients.tools`，没有任何一路会打开 provider 自带的工具。）

另一类服务端工具是可见的：声明成普通 `function`，我们收到 `tool_calls` 后自己去调
provider 的执行端点 —— 那种正常走 `Location` 与 reversibility 判定。两者别混。
哪家属于哪类见 [probes/PROVIDERS.md](../probes/PROVIDERS.md)。

### host 能力差异

stdio 传输只有 server 和桌面侧有，浏览器只能 http。所以 registry 要能表达
**「这个源在这个 host 上不可用」**，而不是假装它存在然后调用时才失败。
（落点是 `ServerConfig::available_on(host)` + `status::Availability`，经 loader 交给宿主。）

### reversibility 等级从哪来

MCP 协议不提供副作用等级。所以：

- 有 `annotations.readOnlyHint == true` 的，映射成 `Pure`
- 其余**一律 `Irreversible`**（没有 annotations、字段缺失、为 false 都算）

不要猜。一个未知来源的 MCP 工具默认可重放，是把数据事故的开关交给第三方。

**没有「本地配置里显式标注」这个逃生口**：`.mcp.json` 的 `StdioServer{command,args,env}` /
`RemoteServer{transport_type,url,headers}` 里没有任何 per-tool 可逆性字段，
[040](issues/040-mcp-seam.md) 的拍板也没给。真要开这个口子，是一次显式的协议扩展，
不是「文档里已经写了」。
