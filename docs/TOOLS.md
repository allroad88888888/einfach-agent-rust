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
    pub reversibility: Reversibility, // Pure | Reversible | Irreversible —— 决策 34 起只喂显示
}

// ③ agent-runtime/src/tool_table.rs —— 宿主侧的判定表，②由它产出
pub struct ToolTable { /* specs + skill registry + 宿主/mcp 的 <tool> → Reversibility 两张映射 */ }
```

**位置是宿主现算出来的；可逆性字段今天也是现算的，但它从决策 34（199 §八）起只是一张
显示标签，不再是任何行为的依据。** `ToolTable::snapshot(name, input)` 造 `ToolCallRequest`
时：`location_of(name)` 按名字前缀推（外加一条白名单，见下）；`reversibility` 三级查表——
`host_reversibility`（宿主声明的映射）→ `mcp:` 前缀查 `mcp_reversibility`（由 server 的
`readOnlyHint` 翻译而来）→ 都没命中退到 `reversibility_of(name)` 那张硬编码的名字规则，
查不到落保守 `Irreversible`（三级优先级写死在 `tool_table.rs` 的 `snapshot` 里，062 已经
落装配）。

**但查得对不等于它决定 undo。** 真正决定 `/undo` 挡不挡的是两条路，都不看这个字段：
执行体在本进程内的（内置截获、M16 扩展包），看它跑完交回的 `Aftermath`——见下面
「Aftermath 三态怎么选」；执行体在别的进程里的（宿主 `web:`/`desk:`、MCP `mcp:`），
交不回函数，看「事实 vs 承诺」判据——见下面 MCP 一节与
[HOST-CAPABILITIES.md](HOST-CAPABILITIES.md) §五。查出来的这张 `reversibility`
从此只喂 CLI/Web 的显示。

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

约定是 `<location-prefix>:<namespace>/<tool>`，MCP 再多一层 server id：`mcp:<server>/<tool>`，
M16 的 Rust 扩展再多一层 pack id：`ext:<pack>/<tool>`。
`location_of` 按前缀推位置：`web:` → `Web`、`desk:` → `Desktop`、`mcp:` → `Server`
（MCP 调用在宿主本地起子进程往返，不需要远端回传）、`ext:` → `Server`
（扩展是编译期依赖，执行体就是本进程里的一个闭包）、其余落 `Server`。

**但只有一族工具遵守这个约定**：

- **遵守**：`srv:fs/read`、`srv:shell/exec`、`srv:skill/read`、`srv:agent/spawn` 等
  由 `ToolTable::builtin()`/`with_*()` 装的；MCP 翻译出来的 `mcp:<server>/<tool>`；
  M10 宿主注入的（前缀由校验**强制**，见下）；M16 扩展包带的 `ext:<pack>/<tool>`
  （前缀由装配期**强制**，见下）。
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

**M16 又添一族（站在「遵守」这一侧）：`ext:<pack>/<tool>`**（决策 29，
[EXTENSIONS.md](EXTENSIONS.md)）。Rust 扩展包
（`ExtensionPack`）带进来的工具，前缀同样**强制**——裸名、`srv:`/`web:`/`desk:`/`mcp:`、
别的包的命名空间，一律在**装配期**拒绝。它跟 M10 那一族的差别只在「最早能报给有权修它的人」
是谁（见下面 §撞名的那张判据表）：注入声明来自一次活的请求，所以整份 400；扩展包是**编译期
依赖、作者是程序员**，所以是 `debug_assert!` + release 丢弃**那一条**（同 `push_spec`/
`with_timed` 的既有哲学，不丢整包——一个钩子名的笔误不该顺手关掉同包里合法的工具）。

包名进名字是为了同一份红利：`ext:` 族与内置五档、`web:`/`desk:` 注入族、`mcp:` 族之间
**结构上撞不了**，两个扩展之间也撞不了，于是三条路都不需要冲突策略。`location_of` 对
`ext:` 落 `Server`，那一条是**显式**写出来的（不是靠兜底）——它是接缝的承诺，
兜底哪天改主意不该把它一起改掉。

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
五条路行为不同，是因为「谁写的、还能不能被告知」不同：

| 路径 | 撞名的作者 | 最早可报点 | 行为 |
|---|---|---|---|
| **M10 宿主声明**（`capabilities`） | 客户端，在一次**活的请求**里 | **就是那次请求** | **整份 400**，会话不创建 |
| **MCP 多 server** | 第三方 server | 配置解析 / 握手 | 重复 server id 是**硬错误**（`ConfigError::DuplicateServerId`）；工具名自带 server id，跨 server **结构上撞不了** |
| **skill 目录装载**（`SkillRegistry::load`） | 部署者，而且**目录顺序是他显式排的** | 没有——先后本身就是他给的信息 | **后来居上**（有意的例外，见下） |
| **内置工具表装配**（`ToolTable::with_*`） | 一半是程序员（五档 + CLI 链），一半是运行时数据（MCP 回包、客户端请求体） | 程序员那半 = **本地测试** | **`debug_assert!` + 看门狗测试**；运行时那半 = **后来的整条不进表**，不 panic |
| **M16 扩展包装配**（`ToolTable::with_extension`） | 程序员（扩展是编译期依赖） | **本地测试**（他自己那次 `cargo test`） | 跟上一行同一套：**`debug_assert!` + release 丢弃那一条**（连它的执行体一起丢，声明与执行路径同进同出）；包名进工具名，跨包**结构上撞不了** |

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

**「跨路径撞名（宿主注入的 `web:foo` × skill 携带的同名 `web:foo`）」这个问题今天不
存在**：064 期 skill 还能携带工具，需要一条过滤规则处理它跟工具表撞名；决策 27
（M15）把「skill 携带可执行工具」整个砍了（`capabilities.skills[].tools` 非空声明即
400，见 §Skills），那条过滤规则与它所属的整套激活时注入机制随
[141](issues/141-remove-activation-subsystem.md) 一起删掉——**没有 skill 携带的工具，
就没有跟工具表撞名这回事**。这条历史决策与它当时的理由留在 issue 064/069 与
ROADMAP.md 决策 21/27 里。

### 第三个维度：调用时机（133）

`ToolTable` 还有一维跟 `location`/`reversibility` 正交、且**跟模型无关**的轴：
`CallTiming`（空 = 模型自主调，今天全部工具都是；`SessionStart`/`TurnEnd` = runtime
建会话/每轮完成后自动调一次）。非空的工具住独立区（`with_timed`/`timed()`），
`specs()`/`declares()`/`snapshot()` 一个字节看不见它——同一条 076 判据的延续：模型
面的表只有一个答案。执行体是**注册时给的本地同步函数**，135/136 的驱动直接调，不
经过 dispatch；v1 因此没有远端/MCP 时机工具，那是将来的显式扩展。

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

### Aftermath 三态怎么选，什么时候该不写还原函数

这个字段决定 `/undo` 会不会在这条 entry 上停下来问。**定错了是数据事故**，不是体验
问题——但它不再决定崩溃恢复要不要重放工具：恢复从不重新执行工具，只重放 journal 里的
状态值，`is_replayable()` 已经随枚举一起删掉（决策 34/199 §八，`value/tool.rs` 有明文）。

决策 34（199）把依据从「注册时填一个等级」换成「执行体跑完之后交回一个 `Aftermath`」
（`crates/agent-runtime/src/undo_hook.rs`）。**是三态，不是 `Option<UndoFn>`**——`Option`
会把「没碰外部世界」和「碰了但撤不回」压成同一个 `None`，而落盘那一位（`Undoability`）
是三态，返回类型必须与它同构：

| `Aftermath` | 判据 | 例 |
|---|---|---|
| `Nothing` | 没碰外部世界，或者只写了本仓自己的 store 状态——回滚 journal 就是补偿 | 读文件、查询、搜索；`srv:agent/spawn`（子 agent 的全部状态活在同一条日志上，父级 entry 回滚时子级原子自动跟着退） |
| `Undo(f)` | 碰了外部世界，且能当场写出一个可靠的还原函数 | 创建资源前记下新资源 id（还原=删除）、覆盖文件前先记旧内容（还原=写回） |
| `Irreversible` | 其余全部——碰了，写不出可靠的还原函数，或者压根没打算写 | 发邮件、支付、删数据、跑 shell |

**拿不准就交 `Aftermath::Irreversible`（等价于以前的「拿不准就是 `Irreversible`」）。**
判据没变，变的是落点：过去是往哪个格子填一个标签，现在是交不交这个函数——**不交函数
就是挡 undo，作者想躲也躲不掉**，比枚举时代更硬：枚举时代填错一个 `Pure` 就能悄悄放行
一次本该拦住的副作用，现在没有对应函数就没有 `Undo` 分支可选，结构上蒙不过去。判错代价
依旧不对称：判成 `Nothing` 的代价是重复发一次邮件；判成 `Irreversible` 的代价只是多问
用户一次。同一个工具的三次调用可以交出三种不同的 `Aftermath`（`fs/write` 建新文件 /
覆盖旧文件 / 写失败，还原方式各不相同）——**可逆性从此是每次调用的属性，不是每个工具
的属性**，枚举表达不了，函数天然表达了。

`undo` 往回走时撞上一条 `Undoability::Blocked` 的 entry（那次调用交回的是
`Aftermath::Irreversible`，或者根本交不出函数）→ 停下，走 `UndoOutcome::Blocked`
（SSE 上是 `undo` 事件里嵌一个 `blocked`），让用户确认「继续（副作用不回滚）」还是取消。
落盘依据是 `EntryMeta.undoability`（决策 199 §九起是三态，之前是 `barrier: bool`）：
宿主派发不可逆工具前调 `Session::mark_no_undo`，随后那条 `tool_result` entry 就带上
`Undoability::Blocked`——**屏障是落盘的**，崩溃重启之后仍然拦得住。交得出还原函数的那种
调用走 `Session::mark_hooked` → `Undoability::Hooked`，undo 路上先调一次钩子、成功了才
回滚状态（顺序不能反，见 STATE-MODEL §「Command log」）。

## Skills

本质是「按需注入 context 的资产」——一段指令 + 若干文件，触发时进 prompt。

**决策 27（M15）换过一次形状**：039 期是「模型经 `srv:skill/activate` 激活 →
正文/自带工具经激活时注入塞进 system 段尾部/中途工具通道」，139/141 换成「索引
常驻、正文按需读、不再携带可执行工具」。下面写的是**今天**这条路，039 期的老
路径与它的理由留在 [ROADMAP.md](ROADMAP.md) 决策 21/27 与
[issue 141](issues/141-remove-activation-subsystem.md) 里，不在这里重复。

### 今天的形状：索引常驻 + 正文按需读

```
srv:skill/index  (138，SessionStart 时机工具)  → 每 skill 一行「id — 描述」，
                                                  135 的开局驱动在建会话那一刻
                                                  跑一次，落进 Session::prefix_chunks()
                                                  （134，会话创建期定死、之后不变）
srv:skill/read   (137，普通工具，进 specs)      → 模型按 id 现取正文，正文经
                                                  tool_result **进对话消息**，
                                                  不进 system 段
```

**为什么这个形状能同时活过三家**（038 的实测数字是这条决策的依据）：索引是
「調用时机 + 详情走工具结果」这个通用机制（决策 27 摘要，见 ROADMAP.md）在
skills 上的第一个落地——工具表的名字前缀（`srv:skill/read` 本身）一个字节不变，
正文永远走「消息尾部追加」这条本来就在做、缓存本就为之设计的路，不再有
「中途插一段 system」这个各家代价不同、DeepSeek 上 120x 归零的动作。真机验收
（139 实做记录）：DeepSeek 十轮 cached/prompt 全部 ≥ 97.8%（含 read 发生的轮）。

**skill 不再携带可执行的工具**：`capabilities.skills[].tools` 非空在声明这一步
就整份 400（140，决策 27）——工具想给某个 skill 用，走 `capabilities.tools` 顶层
声明。`HostSkill.tools` 字段仍然存在（老 journal 反序列化兼容），但没有任何代码
会读它去注入或执行。

### 状态与内容分两边放（没变）

```
SkillRegistry (store 外)             ← 正文，`SkillRegistry::load`/`from_host_skills` 装
Session::prefix_chunks (134)         ← 索引常驻块，会话创建期算一次、落盘、之后不变
Slot::SkillsActive (留壳，141)        ← 只读、没有写入点；agent-cli 的 `/skills` 展示
                                        用它回显老会话的历史激活状态，不影响 prompt
```

`Slot::SkillsActive` 是这条链路唯一留下的「激活」痕迹：039 期活着时它记录「哪些
skill 被激活」，供每轮展开成注入料；141 删了写入点（`activate_skill`/`deactivate_skill`
连同 `SkillError` 一起没了）和唯一的读者（那条每轮把激活集展开成注入料的方法）
——变体本身不能删（红线 4：老会话 journal 里真有 `activate_skill` entry），所以它
是一个**留壳的既有槽位**：老会话恢复时这项数据原样读得出来，但没有任何生产代码
再拿它去组下一轮的请求体。这是**如实的行为变化**，不是半吊子兼容：恢复一个 M13
期真激活过某个 skill 的老会话，继续对话不会再看到那个 skill 的正文。

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

### MCP 工具的可逆性标签从哪来

翻译规则、以及为什么 `readOnlyHint` 从决策 34 起只影响显示、不影响 undo（MCP 协议里
没有撤销这个概念，server 交不出还原函数），见 [MCP.md](MCP.md)
§「枢纽：可逆性不能再从名字推」——不在这里维护第二份正文。
