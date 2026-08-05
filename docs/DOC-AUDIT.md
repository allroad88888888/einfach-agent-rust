# 文档与实现一致性审计

**日期** 2026-08-04 · **范围** `docs/` 十份主文档 + 根 `CLAUDE.md` 对当前代码的逐条核对 ·
**性质** 只读审计，本文件是唯一产出，**没有修改任何现有文档或代码**。

## 怎么读这份报告

每条给出：**文档位置**（file:line + 原文）、**代码事实**（file:line + 原文）、**等级 + 一句话理由**、
**建议**（描述，未实施）。三个等级：

| 等级 | 定义 |
|---|---|
| **危险** | 文档说的和代码做的**相反**。照文档写代码会编译不过、静默走错分支、或做出错误的架构/部署/排期决策 |
| **过时** | 文档描述的是历史状态，代码已演进。照着读不会立刻出错，但会得到一张过期的地图 |
| **小瑕疵** | 措辞、行号、路径、数字对不上，无害 |

另有两个特殊标记：

- **实现比文档更好** —— 不一致是刻意的，实现选了更安全的做法。**该改的是文档，不是代码。**
- **⚠️ 疑似代码问题** —— 这处不一致的根因在代码侧，不是文档跟不上。比文档问题重要，单列 §四。

**统计**：危险 10 条 · 过时 40 条 · 小瑕疵 19 条 · ⚠️ 疑似代码问题 4 条（其中 1 条是**确定的 bug**）。

**快照声明**：审计期间有 5 个 agent 并发在改 `crates/agent-server/`、`crates/agent-runtime/`、
`packages/web/`、`docs/issues/*`（061 与 065 就是在审计过程中落地的）。这些目录下的行号是审计
时刻的值，可能已漂移；**引文本身**比行号可靠，核对时以文本为准。

---

## 零、最危险的三条

### ① `docs/TOOLS.md` 的 `ToolDescriptor` 是一个**从未存在过的类型**

这不只是"一个类型名写错了"。TOOLS.md 全文的论证——位置透明路由、undo 屏障、skill 带工具、
MCP 翻译——**全部建立在"每个工具带着 `location` / `reversibility` / `source` 三个字段"这个前提上**，
而这个前提在代码里不成立。它还污染了另外三份文档和一处代码 TODO（见 D1）。

**文档** `docs/TOOLS.md:9-16`

```rust
struct ToolDescriptor {
    name: String,        // "srv:fs/read" | "web:selection/read" | "desk:shell/exec"
    schema: JsonSchema,
    location: Location,           // Server | Web | Desktop —— router 派发
    reversibility: Reversibility, // Pure | Reversible | Irreversible —— undo / 崩溃恢复
    source: Source,               // Builtin | Mcp(ServerId) | Skill(SkillId)
}
```

**代码** `crates/agent-core/src/value/tool.rs:87-92`

```rust
pub struct ToolSpec {
    pub name: Arc<str>,
    pub description: Arc<str>,
    pub schema: Arc<serde_json::Value>,
}
```

`Source` 枚举全仓零定义（`grep -rn "enum Source\|Source::Builtin\|Source::Mcp\|Source::Skill" crates packages apps` → 0 命中）。
`ToolDescriptor` 全仓唯一出现是 `tool.rs:32` 的一句文档注释在引用它。

### ② 命名空间约定只覆盖一半的工具，另一半靠**硬编码三个名字的白名单**补

**文档** `docs/TOOLS.md:26`

> `<location-prefix>:<namespace>/<tool>`。MCP 来的工具再多一层 server id：`mcp:<server>/<tool>`。

**代码** `crates/agent-runtime/src/tool_table.rs:213-227`

```rust
fn location_of(tool: &str) -> Location {
    if matches!(tool, "ask_user_question" | "browser_action" | "save_file") {
        return Location::Web;
    }
    match tool.split_once(':').map(|(prefix, _)| prefix) {
        Some("web") => Location::Web,
        Some("desk") => Location::Desktop,
        Some("mcp") => Location::Server,
        _ => Location::Server,
    }
}
```

`ToolTable::standard()` / `standard_local()` 装的工具里，`read_file`、`list_files`、`search_files`、
`rg_search`、`apply_patch`、`find_test_lint_commands`、`git_diff_review`、`ask_user_question`、
`browser_action`、`save_file` **一个前缀都没有**（`crates/agent-tools/src/apply_patch_spec.rs:9`：
`name: Arc::from("apply_patch")`；`crates/agent-tools/src/command_discovery_specs.rs:11` 同）。

**为什么危险**：`_ => Location::Server` 是兜底分支。新增第四个前端工具时忘了写 `web:` 前缀、
又忘了加进那个 `matches!` 白名单 → `location_of` 静默返回 `Server` → dispatch 把它送进本地
executor → 模型收到 `unknown_tool`。**不报错、不 panic，只是工具永远调不通**，而且
`tool_table.rs:213` 的三名白名单 与 `crates/agent-tools/src/interaction_specs.rs` 的三个声明之间
**没有任何编译期或测试期的绑定**。

**顺带一个不一致**：M10 的 host capabilities 校验**强制**注入工具必须带 `web:`/`desk:` 前缀
（`crates/agent-server/src/http/capabilities/validate.rs:41,82`：
`const TOOL_PREFIXES: [&str; 2] = ["web:", "desk:"];` / `"必须以 \"web:\" 或 \"desk:\" 开头——注入的工具跑在宿主侧，位置从前缀推"`）。
于是**注入的工具遵守 TOOLS.md 的约定，内置的工具不遵守**，两套规则并存而文档只写了一套。

### ③ `POST /tool_result` 的 body：文档少了必填字段、多了一个被忽略的字段

**文档** `docs/TOOLS.md:46-49`

> ### 回写必须带 epoch
> `POST /tool_result` 的 body 里带上发出时的 epoch。

**文档** `docs/ARCHITECTURE.md:87`

> `POST /sessions/:id/tool_result   { tool_call_id, epoch, result }`

**代码** `crates/agent-server/src/http/routes/tool_result.rs:1-33`

```rust
//! 此端点只把结果送往该 session 的 actor；真正的安全校验在 actor 持有的
//! `RunnerCtx` 中完成，必须精确匹配仍在等待的 `(agent, call_id)`。所以 HTTP
//! 客户端不能指定 epoch，也不能伪造结果填充任意本地工具调用。
struct ToolResultRequest {
    agent: AgentId,
    tool_call_id: ToolCallId,
    result: ToolResult,
}
```

epoch 由服务端在派发时自己记下（`crates/agent-runtime/src/ctx_remote_tools.rs:55`：
`self.pending_remote_tools.0.push(PendingRemoteTool { agent, call_id, epoch, request, deadline });`）。

**等级：危险 + 实现比文档更好**。实现的选择（服务端保管 epoch）更安全——客户端伪造不了世代号。
但文档的形状**照抄会直接失败**：少了必填的 `agent`（serde 会 400），多写的 `epoch` 被静默忽略，
而且会让网关作者以为 epoch 校验是客户端责任。红线 6 依然成立，只是校验点不在 body 上。

**建议**：TOOLS.md §「回写必须带 epoch」改成「回写必须**匹配**在飞的 (agent, call_id)——epoch
由服务端保管，客户端指定不了」，并把红线 6 的落点从 body 挪到 `RunnerCtx`；ARCHITECTURE.md:87
的 body 改成 `{ agent, tool_call_id, result }`。

---

## 一、危险（10 条）

### D1 · `ToolDescriptor` 不存在，且这个幽灵类型污染了四处

**文档** `docs/TOOLS.md:5-16`（见 §零①）

**连带受害**：

| 文档/代码位置 | 原文 | 事实 |
|---|---|---|
| `docs/ARCHITECTURE.md:50` | `agent-mcp/         MCP adapter，产出 ToolDescriptor` | 产出 `Vec<(ToolSpec, Reversibility)>`，见 `crates/agent-runtime/src/tool_table.rs:154` `pub fn with_mcp(mut self, tools: Vec<(ToolSpec, Reversibility)>) -> Self` |
| `docs/STATE-MODEL.md:271` | `答案复用 `ToolDescriptor.reversibility`` | 同上，类型不存在 |
| `docs/TOOLS.md:80` | `Skill 可以携带 tool（`source: Skill(id)`），激活时进工具表` | `Source` 不存在；skill 工具经 `ToolTable::skill_injection`（`tool_table.rs:174`）走 `late_tools`，不是"进工具表" |
| `crates/agent-tools/src/lib.rs:68-70` | `它目前不进入既有 `builtin_specs()`：`agent-runtime` 尚未把它标为 `Pure`…**主工具表迁移到 descriptor 后再显式启用它**` | 代码里有一个**被一个不存在的迁移挡住的 TODO**：`srv:fs/search_files` / `srv:fs/rg_search` 两个纯读工具因此一直没进任何工具表 |

**等级：危险**。照 TOOLS.md 的图设计，会以为 `spec.location` 可读、以为存在一张 per-tool 的
元数据表可查。实际只有 MCP 工具有真正的查表（`ToolTable.mcp_reversibility`，`tool_table.rs:39`），
其余全部按名字硬编码推断（`reversibility_of`，`tool_table.rs:229-270`）。

**建议**：把 TOOLS.md 的代码块改成实际的三层——`ToolSpec`（喂模型的三字段）+
`ToolCallRequest`（发起时快照，带 `location`/`reversibility`）+ `ToolTable`（宿主侧的判定表），
并明写"位置与可逆性**不在 spec 上**，由宿主侧的 `tool_table.rs` 按名字规则 + MCP 映射产出"。
`Source` 一栏整段删掉或标注为"未实现，M10 的 `capabilities` 里由 `origin` 部分承担"。

（`docs/HOST-CAPABILITIES.md:23-26` 已经把这笔账记对了——它是全仓唯一说清这件事的文档，
可以直接把那三行搬进 TOOLS.md。）

---

### D2 · 命名空间约定与硬编码白名单（见 §零②）

**建议**：TOOLS.md §命名空间 补一段"**两套命名并存**"：`srv:`/`web:`/`desk:`/`mcp:` 前缀族
（`builtin`/`with_shell`/MCP/M10 注入）与 web-agent 兼容的无前缀族（`standard_local`/`standard`）。
并把 `location_of` 的三名白名单点名写进文档，说明**加前端工具必须同时改两处**（或建议代码侧
改成从 `interaction_specs()` 反查，消掉这条隐式耦合——那是代码改动，不在本次范围）。

---

### D3 · `POST /tool_result` body（见 §零③）

---

### D4 · `SessionStore` trait 的签名与语义都对不上，且实现**明确否决了**文档的返回类型

**文档** `docs/STATE-MODEL.md:238-246`

```rust
trait SessionStore {
    fn append(&self, id: SessionId, entry: &Entry);
    fn drop_oldest(&self, id: SessionId, count: usize);   // cap 溢出
    fn drop_after(&self, id: SessionId, cursor: usize);   // 新分支覆盖 redo 尾
    fn set_cursor(&self, id: SessionId, cursor: usize);
    fn snapshot(&self, id: SessionId, snap: &Snapshot);
    fn load(&self, id: SessionId) -> Option<(Snapshot, Vec<Entry>, usize)>;
}
```

**代码** `crates/agent-store/src/persist/mod.rs:37-65`

```rust
pub trait SessionStore<K, V, M> {
    fn append(&self, entry: &Entry<K, V, M>);
    fn drop_oldest(&self, count: usize);
    fn drop_after(&self, first_seq: u64, count: usize);
    fn set_cursor(&self, cursor: usize);
    fn snapshot(&self, snap: &Snapshot<K, V>);
    fn load(&self) -> LoadOutcome<K, V, M>;
}
```

`persist/mod.rs:60-63` 明写：

> 三态见 [`LoadOutcome`]——`Option` 曾经把「文件不存在」和「有会话但拒绝加载（中部损坏/不变量破坏）」
> 都压缩成 `None`，宿主没法区分，"没有会话"与"有会话但读不出来"对宿主是两件完全不同的事：
> 前者开新会话是对的，后者必须硬失败

**等级：危险 + 实现比文档更好**。三处实质差异：
① 没有 `SessionId` 参数——一个 store 实例服务一个 session（`Jsonl` 就是一个文件）；
② `drop_after(first_seq, count)` 与文档的 `drop_after(cursor)` **语义不同**，照文档理解会丢错条目；
③ `load` 三态是刻意的，文档的 `Option` 是被否决过的设计。

**建议**：整块换成实际签名，并把 `LoadOutcome` 三态的理由（"没有会话"vs"读不出来"）搬进
STATE-MODEL——那是一条真正的设计结论，值得留在主线文档里。

---

### D5 · `Entry.owner` 字段不存在，多租户"不用迁 schema"的承诺是反的

**文档** `docs/ARCHITECTURE.md:145-147`

> **无鉴权 ≠ 无身份。** server 仍然要知道「这是谁的 session」用于隔离与审计归属。
> 做法是信任上游传入的 `X-Agent-Tenant-Id` / `X-Agent-User-Id`，读不到就落 `anonymous`。
> 于是企业加鉴权时 server 一行不改，`Entry.owner` 字段现在就留着，以后要多租户不用迁 schema。

**文档** `docs/STATE-MODEL.md:82-90` 的 `Entry` 结构里同样列着 `owner: Option<String>`。
**文档** `docs/INTEGRATION.md:70-71` 把它升级成了完成时：

> 这跟 ARCHITECTURE **已有的** `X-Agent-Tenant-Id` / `X-Agent-User-Id` 透传是同一套思路

**代码** `crates/agent-core/src/command/meta.rs:21-37`

```rust
pub struct EntryMeta {
    pub turn_id: u64,
    pub epoch: Epoch,
    pub label: &'static str,
    pub barrier: bool,
}
```

没有 `owner`，也没有 `agent`。`agent-store` 侧的 `Entry<K, V, M>` 只有 `{ seq, meta, changes }`
（`crates/agent-store/src/history/log.rs:33-37`）。`log.rs:28` 的注释还留着计划：
`M` 是元数据的占位。agent 侧未来往里放 turn_id / epoch / owner / agent / label`——「未来」没到。

server 端从不读那两个 header：全仓 grep 只在 `crates/agent-server/src/http/routes/sessions.rs`
的一句注释里出现（`本 issue 也不做多租户鉴权（X-Agent-Tenant-Id 是未排期项）`），
以及 Java 网关的注释里。`anonymous` 全仓零命中。Java 网关**确实**全量转发 header
（`examples/java-gateway/src/main/java/com/example/agentgateway/proxy/HopByHopHeaders.java`），
但 Rust 侧把它们丢在地上。

**等级：危险**。这是一句**反向承诺**：文档说"以后要多租户不用迁 schema"，实际必须迁——
`EntryMeta` 是 `Serialize` 的落盘结构，加字段就是改快照/日志格式。企业照这段做多租户规划会踩空。

**建议**：ARCHITECTURE §边缘无关 那两句改成明确的未来时 + 未排期标注；`Entry.owner` 那半句
删掉或改成"多租户落地时要在 `EntryMeta` 上加 `owner`，**这是一次落盘 schema 变更**"。
INTEGRATION.md:70-71 的「已有的」改成「**规划的**（server 侧尚未实现）」。
STATE-MODEL 的 `Entry` 代码块同步改（见 S12）。

---

### D6 · §多副本粘性路由：`trait SessionRegistry` / `PodAddr` / 转发逻辑一行都没有

**文档** `docs/ARCHITECTURE.md:127-134`

```rust
trait SessionRegistry {
    fn owner(&self, id: SessionId) -> Option<PodAddr>;
}
```
> `LocalRegistry` 永远命中自己（单副本，转发分支是死代码），多副本时换 `RedisRegistry`,
> 网关和前端零改动。**转发逻辑现在就写，registry 抽象现在就留。**

**代码** `crates/agent-server/src/registry/mod.rs:2-5` 直接反驳这段话：

> 内存表——`ARCHITECTURE.md` §「多副本时的粘性路由」画的 `trait SessionRegistry`
> （`fn owner(&self, id) -> Option<PodAddr>`）是 **M4 后 `RedisRegistry` 落地时才长出来的接缝**，
> 这里先把单机版的语义做对：`open`/`get`/`close`……

`PodAddr` / `LocalRegistry` / `RedisRegistry` 全仓零定义。`docs/issues/README.md` 的 M6 未排期段
也把 `多副本的 RedisRegistry` 列为延后。

**等级：危险**。它是一条**部署决策依据**："转发逻辑现在就写"会让人以为多副本只差换个 registry
实现，实际连跨 Pod 转发（含 SSE 反代）都还没有。

**建议**：整节改成"**设计留位，未实现**"，并把 `registry/mod.rs:2-5` 已经写好的那句话搬上来
（代码注释比文档诚实，让文档跟上代码）。

---

### D7 · SSE 事件名：ARCHITECTURE 列的四个名字**一个都不存在**

**文档** `docs/ARCHITECTURE.md:85`

> `GET  /sessions/:id/events        SSE：token / tool_call / state / undo_blocked`

**文档** `docs/TOOLS.md:37-38`

> `Web` / `Desktop` —— 往 SSE 上扔 `tool_call` 事件，把 `toolcall.<id>.result` 置 `Pending`

**代码** `packages/protocol/src/generated/SessionEvent.ts:24`（由 Rust 生成，
源在 `crates/agent-server/src/event/mod.rs:85-145`）

```ts
export type SessionEvent = { "type": "text_delta", … } | { "type": "thinking_delta", … }
  | { "type": "tool_call_started", … } | { "type": "preflight_drift_alert", … }
  | { "type": "transport_trouble", … } | { "type": "tool_executing", … }
  | { "type": "tool_executed", … } | { "type": "turn_guard", … } | { "type": "notice", … }
  | { "type": "undo", … } | { "type": "redo", … } | { "type": "lagged", … }
  | { "type": "session_died", … } | { "type": "gap", … } | { "type": "agent_tree", … }
  | { "type": "orphaned_child", … };
```

`undo_blocked` 实际是嵌套的：`{"type":"undo","data":{"type":"blocked",…}}`
（`packages/protocol/src/generated/UndoOutcome.ts:5`）。

**等级：危险**。照 ARCHITECTURE 写前端事件 `switch`，**四个 case 一个都命中不了**，
而且不报错——事件静默落进 default 分支。

**建议**：ARCHITECTURE §传输 那一行改成"事件类型见 `packages/protocol/src/generated/SessionEvent.ts`
（Rust 生成，不手维护）"，不要在文档里维护第二份清单——这正是 ARCHITECTURE §协议类型 自己说的
"线上协议存在两份手写副本是企业级项目最常见的腐坏源"。TOOLS.md:37 的 `tool_call` 改成 `tool_executing`。

---

### D8 · 多来源撞名的冲突策略"现在定死"了，但从没写下来过；而代码里有**三套互相矛盾**的做法

**文档** `docs/TOOLS.md:29-30`

> 企业级多来源一定会撞名——两个 MCP server 各有一个 `search`，前端和后端各有一个 `read_file`。
> **冲突策略拖到后面改就是破坏性变更，现在定死。**

——然后全文再没有出现过任何冲突策略。

**文档** `docs/TOOLS.md:85`

> 内置 / 项目 / 用户 / 远端四个来源，用**和 tool 同一套** merge + 冲突策略。

**代码** `crates/agent-runtime/src/skill/mod.rs:57-59` 引用了一条 TOOLS.md 里不存在的规则：

> 从若干来源目录装载（内置 + 项目 `./skills/`……）。**合并**：后一个目录里同名 skill 覆盖前一个
> （跟工具表「后来居上」一套规则，**TOOLS.md §多来源**）。

TOOLS.md 没有 §多来源 这一节，也从没写过"后来居上"。

**三套做法**：

| 路径 | 撞名时 | 出处 |
|---|---|---|
| 内置工具表 | **静默重复**——`with_*` 一路 `push`，不检测不去重；`declares()` 用 `.any()` 取第一个 | `crates/agent-runtime/src/tool_table.rs:60-160, 183-185` |
| skill 目录装载 | **后来居上** | `crates/agent-runtime/src/skill/mod.rs:57-59` |
| M10 注入声明 | **一律拒绝** | `crates/agent-server/src/http/capabilities/validate.rs:88-97`：`工具名 "{name}" 被重复声明（这一次在 {origin}）——重名一律拒绝，不做「后来居上」` |
| MCP 多 server | server id 硬去重（重复 server id 是**硬错误**，不是后来居上） | `crates/agent-mcp/src/config.rs:141-149` `ConfigError::DuplicateServerId` |

**等级：危险**。文档承诺"现在定死"的东西不存在，而四条路径各行其是。撞名时的行为完全取决于
走哪条路，没人能从文档推出来。

**建议**：TOOLS.md 补一节把四条策略如实写下（含"内置工具表目前**不检测**重名"这个事实），
并把 `validate.rs:10` 那句理由（"宿主自己都没想清楚要哪个，server 替它选一个只会……"）搬上去——
它是四者里唯一有论证的。`skill/mod.rs:58` 那处对 TOOLS.md 的引用是悬空的，也该记一笔。

---

### D9 · `HOST-CAPABILITIES.md` §九 的 issue 编号指向**被废弃的那一代**，且与现行计划正面撞号

**文档** `docs/HOST-CAPABILITIES.md:254-263`

```
060(远端挂死,前置) ─┬→ 061(capabilities 协议+工具装配) ─┬→ 063(前端注入+MCP 客户端) → 064(真机,终点)
                    └→ 062(skill 注入+唤醒 server skill) ┘
```
> | **061** | `POST /sessions` 收 `capabilities.tools`、`OpenSpec` 带上它、per-session `ToolTable` 追加 + 可逆性映射、名字前缀校验 | 060 | **opus** | ✅（红线 11） |
> | **063** | 前端接线：注入 capabilities、**执行 remote tool 并 `POST /tool_result`**、MCP 客户端（形态 B） | 062 | sonnet | — |

**现行计划** `docs/issues/README.md:310-326`（九个 issue，与上表**编号相同含义不同**）

```
  ├─ Rust 线： 061(协议+校验) → 062(per-session装配) → 064(skill注入+唤醒)
  │                              └∥ 063(红线11确定性锁，与062并行)
  └─ 前端线： 065(注入声明) → 066(执行remote tool) → 067(MCP客户端)
```

> | [061](061-capabilities-protocol.md) | `capabilities` 协议类型 + 名字校验（**纯数据零 IO**） | — | sonnet | — |
> | [063](063-capabilities-determinism.md) | **红线 11 字节确定性锁**（独测，与 062 **并行**） | 061 | **opus** | ✅ 本体 |

**文件系统里两代都在**：`061-capabilities-protocol.md` 与 `061-capabilities-tools.md`、
`062-capabilities-assembly.md` 与 `062-capabilities-skills.md`、
`063-capabilities-determinism.md` 与 `063-frontend-inject-and-mcp.md`、
`064-capabilities-skills.md` 与 `064-host-capabilities-dogfood.md` —— 四组重号并存。
（`038-frontend-tools.md` 与 `038-skill-injection-probe.md` 是第五组，历史更久。）

**等级：危险**。这不是排版问题，是**派活会派错**：照 HOST-CAPABILITIES §九 领 061 的 agent
会拿到 opus + 独测 + 工具装配，而现行 061 是 sonnet + 无独测 + 纯数据零 IO；照它领 063
会去写前端和 MCP 客户端，而现行 063 是红线 11 的字节确定性锁。

**建议**：§九 整段换成 `docs/issues/README.md` §M10 的表，并加一句"**唯一真值在
`docs/issues/README.md` §M10**"；四组废弃的 issue 文件删掉或改名加 `-superseded` 后缀。
**注意 `docs/issues/` 正在被并发修改，这条可能已在处理中。**

---

### D10 · `capabilities` 在 `existing`/`recovered` 时怎么办：文档说"我倾向报 400"，代码+测试已经把"静默接受 200"钉死了

**文档** `docs/HOST-CAPABILITIES.md:224-228`

> **要拍的子问题**：`existing`/`recovered` 时传了 `capabilities` 该**静默忽略**还是**报错**？
> - 静默忽略：客户端重连时无脑带上声明就行，简单；但**「我以为注册上了其实没有」会变成难查的行为差异**。
> - **报 400**：调用方立刻知道「这个会话已经存在，你的声明没被采纳」。**我倾向这个**——

**代码** `crates/agent-server/src/http/routes/sessions.rs:110-121`：校验完 `capabilities` 之后
直接落进 `Some(SessionQuery::Alive(_)) => CreateSessionOutcome::Existing`，没有任何"已存在就拒绝
声明"的分支。

**测试已经把它钉死** `crates/agent-server/tests/http_capabilities_declaration.rs:52-54`

```rust
    let again = create(addr, json!({ "id": CHAT_ID, "capabilities": valid_capabilities() }));
    assert_eq!(again.status, 200, "{}", again.body);
    assert_eq!(support::extract_json_string_field(&again.body, "outcome"), "existing");
```

**等级：危险**。文档倾向的方案与实现选的方案**相反**，而且实现选的正是文档自己预言的
失败模式（"我以为注册上了其实没有"）。062 会继承这个契约，再不拍就永久固化。
两边都不知道对方——文档还标着"待拍"，测试已经断言了另一个答案。

**建议**：把这个子问题在文档里拍掉。若拍 400，同一次改动必须改
`http_capabilities_declaration.rs:52-54`；若拍静默接受，把文档里"我倾向报 400"的段落改成
拍板记录并写清理由（可能是"重连时无脑带声明"的实用性压过了显式性）。

---

## 二、过时（40 条）

### `CLAUDE.md` / `ROADMAP.md`

**S1 · `CLAUDE.md` §当前状态整段停在 M4**

`CLAUDE.md:50-58`

> **M1–M4 全部完成**（2026-08-01/02）：九个 crate + 双 workspace + Tauri 桌面、954 测试。……
> Java 参考网关随仓（examples/，**本机未构建验证**，README 有诚实声明）。

M5（`docs/ROADMAP.md:176`）、M6（`:77`）、M7（`:56`）、M8（`:148`）、M9（`:114`）全部标"已完成"，
M10 已设计且 061/065 已落地。Java 网关已构建验证——`examples/java-gateway/README.md`
「Build verification」段：

> `mvn -q package` has been run … with Temurin/Homebrew OpenJDK 21 and Maven 3.9.15, and it succeeds…
> **This supersedes the earlier "no JDK on this machine, source-reviewed only" note.**

`examples/java-gateway/target/agent-gateway-0.0.0-reference.jar` 实际存在。

**建议**：改成"M1–M9 完成，M10 进行中"；测试数换成当前值或直接删掉（它必然过时，而 CLAUDE.md
自己第 60 行就写着"别信文档对『已完成』的描述"）；Java 网关那句改成"已用 OpenJDK 21 构建验证（058）"。

**S2 · crate 数量：三处文档说 6 或 9，实际 10**

`CLAUDE.md:52`「九个 crate」· `CLAUDE.md:75`「Cargo workspace 在 `crates/`（六个 crate）」·
`docs/ROADMAP.md:44`「`crates/` 六个 crate（M1 产物，见下）」· `docs/ARCHITECTURE.md:44-61` 树里列 6 个。

`Cargo.toml:3-14` 是十个。**真实后果**：`agent-runtime` 在架构图上完全隐身，而它恰恰是工具表、
dispatch、runner pump、skill registry 的所在地——本次审计里裂缝最多的那个 crate。
`agent-tools` / `agent-transport` / `agent-cli` 同样缺席。

**S3 · `ROADMAP.md:46` 的 `docs/` 清单少列 M6–M10 的五份接缝文档**（MCP / OBSERVABILITY /
ORCHESTRATION / INTEGRATION / HOST-CAPABILITIES）。

### `ARCHITECTURE.md`

**S4 · `packages/client` 与 `packages/ui` 不存在**（`ARCHITECTURE.md:55-57`）。
`ls packages/` 只有 `protocol` 和 `web`；传输与渲染都在 `packages/web/src/`。

**S5 · §传输的端点表少四个**。`ARCHITECTURE.md:84-91` 列六个，
`crates/agent-server/src/http/routes/mod.rs:20-31` 有十条——缺 `POST /sessions`、
`GET /sessions/{id}`、`GET /sessions/{id}/agents`（048）、
**`GET /sessions/{id}/events/poll`（056，M9 的拉取式端点，决策 25）**。最后一条尤其要补，
它是 M9 企业集成的核心接缝。

**S6 · §Java 网关整节描述的是 M9 之前的透传形态**。`ARCHITECTURE.md:154-156` 说"一个 SSE
透传的 `@GetMapping`、五个 POST 转发"；实际是
`examples/.../proxy/AgentProxyController.java:35` 的 catch-all `@RequestMapping("/agent/**")`
加一个**自己产生** SSE 的 `@GetMapping`（`AgentSseController.java:30`），后者用 25s 长轮询去拉
上游 `GET /events/poll`（`:64`）。按 `docs/ROADMAP.md:37`（决策 25），§「SSE 代理的四个坑」
在这条链上"**结构上不存在**"。文档也完全没提网关现在负责用 `ProcessBuilder` + `--ready-file`
拉起并托管 Rust 子进程（`.../runtime/AgentServerProcess.java`）。
**注**：参考实现**仍然**用 WebFlux（`pom.xml:44`），所以"必须用 WebFlux"字面没错，但它的
**理由**已不适用；example 自己的 README 对此是诚实的
（`examples/java-gateway/README.md` 「Why this remains WebFlux, but is no longer required by Rust SSE proxying」）。

**S7 · `agent-mcp` 产出的不是 `ToolDescriptor`**（`ARCHITECTURE.md:50`，见 D1 表）。

**S8 · 桌面独有能力（`location: Desktop` 的工具）一个都没注册**。`ARCHITECTURE.md:182` 说
"桌面独有能力（fs、shell）以 `location: Desktop` 的 tool 注册进去"；全仓没有任何 `desk:` 工具被
注册，`crates/agent-server/src/bootstrap.rs:124` 明写 `// `desk:` 工具。真需要调时再往
`BootstrapOptions` 加，不提前造。`。`"desk:` 字面量只出现在转义测试、M10 注入校验及其 fixture 里。

**S9 · `typeshare` 从未被使用**（`ARCHITECTURE.md:215`）。`Cargo.toml:48` 只有 `ts-rs`。
协议生成一致性现在由本地收工命令验证：
`cargo test -p agent-server --features ts`。仓库不再配置托管 CI。

**S10 · 事件环形缓冲不在 actor 里**。`ARCHITECTURE.md:94` 说"**actor 内**保留一个有界事件环形
缓冲"；ring 在 HTTP 层的 per-session hub（`crates/agent-server/src/http/hub/ring.rs`，默认 256 帧，
`http/config.rs:31`），`crates/agent-server/src/actor/` 下 grep `ring` 零命中。
**位置决定语义**：ring 是进程内的，跨进程重启后 `Last-Event-ID` 补不回旧帧——
`docs/ROADMAP.md:130-132` 已把这条写成"极易被误读成缺陷"的观察。

**S11 · §关键判断 2「料单由引擎增量维护」目前只对 `messages` 成立**（见 S13）。

### `STATE-MODEL.md`

**S12 · source 槽位表：三行不存在、两处改名、八个已落地槽位缺席**

`STATE-MODEL.md:26-34` 列 `config` / `messages` / `system_base` / `skills_active` /
`tools_registry_version` / `turn_status` / `toolcall.<id>.result`。

`crates/agent-core/src/graph/slot.rs:169-181`：

```rust
pub const ALL: [Slot; 11] = [
    Slot::Messages, Slot::Status, Slot::ToolSlots, Slot::PrevPrefix,
    Slot::NextMessageId, Slot::TurnsUsed, Slot::MaxTurns, Slot::RetriesUsed,
    Slot::MaxRetries, Slot::ToolsAllowed, Slot::SkillsActive,
];
```

`slot.rs:20-24` 自己记了账：`config` / `system_base` / `tools_registry_version` 至今没有写入点。
缺席的八个里 `ToolsAllowed` 身兼"活名单"（`slot.rs:60-66`：`Null` = 不在活名单上），
是 spawn/despawn/undo 的关键槽位，文档里完全不存在。

**S13 · Derived atom 清单六项，实际只有一项**

`STATE-MODEL.md:41-48` 列 `prompt.system` / `prompt.payload` / `turn.pending` / `turn.can_submit` /
`ui.token_estimate` / `ui.timeline`；`crates/agent-core/src/graph/slot.rs:186-191`：

```rust
pub enum DerivedKey {
    /// 「本 agent 的工具槽全都不是 `Pending` 了吗」。003 预言的那个 derived。
    ToolsConverged(AgentId),
}
```

实际 `Ingredients` 由宿主每轮现组（`crates/agent-runtime/src/provider_call.rs` 的 `start`），
skill 正文/工具由 `ToolTable::skill_injection` 现算（`tool_table.rs:174-176`），不经原子图。

**等级：过时，但这是架构叙事层面的过时**——ARCHITECTURE §一句话 和 §关键判断 2 的说服力
建立在"prompt 组装是 derived"上，那部分没落地。建议两份文档都加一句"设计目标；当前只有
`ToolsConverged` 落地"。这不是否定设计，是别让读者以为已经这样了。

**S14 · `Entry` 结构的字段与实际不符**

`STATE-MODEL.md:81-91` 的 `Entry { seq, turn_id, epoch, owner, agent, label, changes }` vs
`crates/agent-store/src/history/log.rs:33-37` 的 `Entry<K,V,M> { seq, meta, changes }` +
`crates/agent-core/src/command/meta.rs:21-37` 的 `EntryMeta { turn_id: u64, epoch: Epoch,
label: &'static str, barrier: bool }`。

差异：① 泛型三段式（agent 词汇全在 `M` 里，因为 `agent-store` 不许 import `agent-core`）；
② `owner` 不存在（D5）；③ **`agent` 字段不存在**；
④ **`barrier: bool` 文档没写**——而它是 undo 屏障的**唯一**落盘依据（`meta.rs:31-37`）。

**S15 · 日志上限"默认 100 条"—— 结构层默认是无上限**

`STATE-MODEL.md:103` vs `crates/agent-store/src/history/cap.rs:9-13`：

> `History::new()` 的 `cap` 仍然是 `None`（无上限），**不**在这里硬编码「默认 100」。
> issue 原文的「默认 100」是**会话层的策略**，不是日志结构本身的常量

数字对，层次错。建议加半句"`History` 本身默认无上限，100 是会话层设的"。

**S16 · `read_ancestor` / `read_descendant` 的签名**

`STATE-MODEL.md:191-194` 的两参无 `Result` vs
`crates/agent-core/src/command/cross_read.rs:59-64, 79-84` 的
`fn read_ancestor(&self, reader: &AgentId, target: &AgentId, slot: Slot) -> Result<AgentValue, ReadDenied>`。
`Result` 那一半值得留在文档里——"越界是被显式拒绝（`ReadDenied::NotAnAncestor`），不是返回默认值"
是红线 10 的落点。

**S17 · `ToolCallSlot::Request` 不存在；§中断语义整表因此没有落盘依据**

`STATE-MODEL.md:117-120` 写 `ToolCall(AgentId, ToolCallId, ToolCallSlot),   // Request | Result`，
`:126-128` 说 `Request` 存发起当时的 `Location` / `Reversibility`。
`crates/agent-core/src/graph/slot.rs:91-99` 只有 `Result`，并说明了为什么：

> `Request`……要等**持有工具表的宿主**来记——core 没有工具表，现造一份占位快照是编造
> （002 合并时的裁决：假的 `Irreversible` 会让 undo 白拦一次 `fs/read`，正是静默错值）

**下游后果**：`STATE-MODEL.md:271-280` §中断语义 整张表的输入（发起当时的可逆性）不进快照，
只活在宿主内存（`crates/agent-runtime/src/ctx_remote_tools.rs:55`）。
**这不等于红线 6 或 undo 屏障有洞**——屏障位 `EntryMeta.barrier` 是落盘的。有洞的是崩溃恢复
这一条，见 §四·C。

### `INVARIANTS.md`

**S18 · 红线 10 的可读 slot 集合**

`INVARIANTS.md:157-159` 写"往上读 `messages` / `config` / `skills`，往下读 `status` / `result` / `usage`"；
`crates/agent-core/src/graph/visibility.rs:144-151`：

```rust
assert_eq!(slots_with(Visibility::Upward),   vec![Slot::Messages, Slot::SkillsActive]);
assert_eq!(slots_with(Visibility::Downward), vec![Slot::Status, Slot::ToolsAllowed]);
```

`config` / `result` / `usage` 三个槽位不存在。`visibility.rs:39-43` 自己记了这笔账。
**规则本身完全成立**（不相交 + 穷举 match 无 `_` 通配 + 集合性质测试），只是举例的槽位名过期。

### `ADAPTER.md`

**S19 · `Ingredients` 缺 `late_system`**。`ADAPTER.md:48-56` 七个字段，
`crates/agent-providers/src/lib.rs:42-61` 八个。缺的是 039 加的：

```rust
    /// **本轮激活的 skill 正文段**（039）——跟 `late_tools` 一样「宁可分不可合」：
    /// Kimi/GLM 把它挂成消息级 system（~100% 保前缀，免费），DeepSeek 拼进顶层
    /// system 段尾部（插新 system 消息 038 实测 120x 归零，改段尾保 ~91%）。
    pub late_system: &'a [SystemChunk],
```

它是 §「宁可分，不可合」那条规则**最好的实证**（一家免费、一家 120x），文档里缺席可惜。

**S20 · `Adjustment` 少两个变体，字段类型全漂**

`ADAPTER.md:122-131` 四个 vs `crates/agent-core/src/seam.rs:36-54` 六个：

| | 文档 | 代码 |
|---|---|---|
| 缺失 | — | `ThinkingDisabledForToolChoice` |
| 缺失 | — | `LateSystemReshapedPrefix { est_cost_multiple: f32 }`（039） |
| 类型 | `ToolChoiceDowngraded { wanted: ToolName, used: &'static str }` | `{ wanted: Arc<str>, used: Arc<str> }` |
| 类型 | `LateToolsForcedIntoPrefix { count: usize, … }` | `{ count: u32, … }` |
| 类型 | `ToolsTruncated { kept: usize, dropped: usize }` | `{ kept: u32, dropped: u32 }` |

`ToolName` 类型全仓不存在。`ThinkingDisabledForToolChoice` 特别值得补——它正是 §料单 表格里
`MustUse` 那一行说的"有一家要**先关思考**才能传"，文档说了现象却没给出对应的 `Adjustment`。

**S21 · `Capabilities` 已从 `agent-providers` 消失，而这个名字被 M10 占用了**

`ADAPTER.md:193` 的类型归属表把 `Capabilities` 列在 `agent-providers`；那里没有这个类型。
全仓唯一的 `Capabilities` 是 M10 新造的、**完全无关**的一个：
`crates/agent-server/src/http/capabilities/mod.rs:38`（宿主注入的工具/skill/MCP 声明）。

**等级：过时 + 实现比文档更好**。023 大概率是**执行了 ADAPTER 自己定的判据**
（`ADAPTER.md:174-178`：「每一位至少两家取值不同且 adapter 内部真的用到，两条有一条不满足就删」）
把它删干净了——文档给了规则，实现遵守了规则，只有类型表没跟上。
**建议**：§「`Capabilities` 还在，只是不出 adapter」改成"023 按下面这条判据逐位过了一遍，
**没有一位同时满足两条，于是整个类型删掉了**"——这是这份文档最有说服力的一段实证，
现在反而写成了它还在。另外强烈建议加一句消歧指向 M10 的同名类型。

### `TOOLS.md`

**S22 · Skills 的 atom 链三个节点里两个不存在**。`TOOLS.md:74-81` 的
`skills.active → prompt.system → prompt.payload` 与 `tools.registry_version` bump：
只有 `Slot::SkillsActive` 存在（见 S12/S13）。实际是每轮 `ToolTable::skill_injection(active)`
现算出 `(late_system, late_tools)`（`tool_table.rs:174-176`）。**结论不变**（"换一个 skill
不碰消息序列化"依然成立），只是路径不是 derived atom。

**S23 · Skills 的四个来源**。`TOOLS.md:85` 说"内置 / 项目 / 用户 / 远端四个来源"；
`SkillRegistry::load(dirs: &[PathBuf])`（`crates/agent-runtime/src/skill/mod.rs:60`）只吃目录，
没有"远端"这一路。冲突策略见 D8。

**S24 · MCP 可逆性的"本地配置显式标注"逃生口不存在**

`TOOLS.md:116`：`其余**一律 `Irreversible`**，除非本地配置里显式标注`
`docs/MCP.md:51`：`- 其余**一律 `Irreversible`**（无 annotations、字段缺失、为 false），除非本地配置显式标注`

`crates/agent-mcp/src/translate.rs:37-40`：

```rust
    let reversibility = match &tool.annotations {
        Some(annotations) if annotations.read_only_hint == Some(true) => Reversibility::Pure,
        _ => Reversibility::Irreversible,
    };
```

配置里没有任何 per-tool 可逆性字段：`StdioServer` 是 `{command, args, env}`（`config.rs:60-64`），
`RemoteServer` 是 `{transport_type, url, headers}`（`:77-82`）。`docs/issues/040-mcp-seam.md:19`
的拍板也没有逃生口。**建议**：两处都删掉那个从句，或挪进"M6 明确不做"。

**S25 · 服务端工具的"会话级开关"没有实现**。`TOOLS.md:98-100` 写的是"正确的建模"
（设计判断，不是完成宣告，所以危害小）。`SessionConfig` 里没有对应字段，三家 adapter 的
`encode` 都只发 `Ingredients.tools`。建议加"（尚未实现——三家 adapter 目前都不发 provider
自带工具，所以这个开关还没有必要）"。

### `MCP.md`

**S26 · "atom / 快照里只有 server 的配置与逻辑标识"—— 实际什么都没有**

`MCP.md:65` 说 atom/快照里有 server id、命令行、可用性位。`crates/agent-core/src/graph/slot.rs:43-87`
的 `Slot` 没有任何 MCP 变体，`AtomKey` 只有两个变体。CLI 把 `McpStatus`/`McpConfig` 纯放进程内存，
每次启动重读 `.mcp.json` 重连（`crates/agent-cli/src/main.rs:116-123`）。
同样的过度声明也出现在代码注释里：`crates/agent-mcp/src/config.rs:46-47`
`server 的逻辑标识与命令行会进 atom/快照（红线 3）`——也是假的。
**建议**：改成"MCP 的配置与可用性**不进 store**，只在进程内；红线 3 在这里由『core 不依赖
agent-mcp』结构性保证；崩溃恢复靠每次启动重读 `.mcp.json` 重连"，并顺手改 `config.rs:46-47`。

**S27 · "`run_effect` 分三路 …… MCP 加第四路"—— 现在有七条分支，MCP 是第五条**

`MCP.md:91-92` vs `crates/agent-runtime/src/dispatch.rs:88-147`：依次是 `SPAWN_TOOL`(:88)、
`COLLECT_TOOL`(:96)、`STATUS_TOOL`(:105)、`SKILL_ACTIVATE|SKILL_DEACTIVATE`(:111)、
`mcp:`(:130)、远端 `web:`/`desk:`(:142)、兜底 `tool_exec::execute`(:147)。
代码自己管远端那条叫"第五路"（`dispatch.rs:133`）。签名也是
`tool_exec::execute(ctx, agent, call_id, request, epoch)`（`tool_exec.rs:16-22`），不是 `execute(ctx.fs)`。

**S28 · "registry 带可用性位"—— `McpRegistry` 没有可用性字段**

`MCP.md:110` vs `crates/agent-mcp/src/registry.rs:20-23`：

```rust
#[derive(Default)]
pub struct McpRegistry {
    clients: Mutex<HashMap<String, McpClient>>,
}
```

可用性实际住在另外三处：`availability::Host::supports`（`availability.rs:37-43`）、
`config::ServerConfig::available_on`（`config.rs:96-98`）、
`status::Availability`/`ServerStatus`（`status.rs:13-36`），经 `LoadOutcome.servers` 交出来
（`loader.rs:50-51`）。

**S29 · 浏览器 host 的 MCP 方案已被 M10 决策取代**

`MCP.md:110-111` 与 `:136` 说"等 http 传输来了，浏览器 host 才长出远端 server"；
`docs/HOST-CAPABILITIES.md:110` §七 **否决**了形态 A（server 去连），**采用**形态 B
（前端自己连，注入 `web:mcp-<server>/<tool>` 工具，走既有 remote 通道），并明写
"这恰好补上 M6 明确延后的那一项"。`docs/issues/README.md:328-332` 同。
`Host::Browser`（`availability.rs:26-27`）目前零生产调用方。
**等级：过时**——照 MCP.md 走会去建一个已被否决的形态。

### `OBSERVABILITY.md`

**S30 · "可观测性不是 agent 的视角"已被 051 推翻，而自查表会把已落地的代码判成 bug**

`OBSERVABILITY.md:36-37`

> agent 之间**不**经这个接口互看——那还是红线 10 管的横读禁令。**可观测性是宿主 / UI 的
> 视角，不是 agent 的视角**

`OBSERVABILITY.md:108` 自查表：`| agent 之间经这个接口互看 | 横读（红线 10） | 可观测性是宿主 / UI 视角，不是 agent 的 |`

`crates/agent-runtime/src/status_tool.rs:4-10`

> # 它是 M7 那棵活树的**模型侧**对偶
> 046 的 [`Session::agent_tree`] 已经把整棵活 agent 树摆成一份纯派生快照，
> 047/048/049 把它送给**人**看……这个工具把同一份快照送给**模型**看

051 ✅（`docs/issues/README.md:217`），接在 `dispatch.rs:105-107` + `ToolTable::with_status`。
**不变量本身仍成立**——status 收窄到调用者的后代（`status_tool.rs:24-26`），是红线 10 的下读。
**但那一行自查表会把已拍板已落地的代码判成红线违规**，这是它比普通过时更值得改的原因。
**建议**：改成"默认视角是宿主 / UI；M8 的 `srv:agent/status`（051）把同一份 `agent_tree()`
也给了模型，但**收窄到调用者的后代**——仍是下读，横读（兄弟/祖先）依旧禁止"；
自查表那一行的症状改成"agent 读到了兄弟 / 祖先"。

### `HOST-CAPABILITIES.md`

**S31 · 两节都叫 `## 八`**。`:159` 是"server 形态下 skill 从没装载过"，`:170` 是"安全：暂缓讨论"。
于是所有后续的"§八"引用（如 `:277` `**安全（§八）暂缓**`）都是歧义的。

**S32 · §一 现状表的「声明 ❌ 完全没有」已被 061 推翻**

`:17` 说 `POST /sessions` 的请求体只有 `id` 和 `session_path`；
`crates/agent-server/src/http/routes/sessions.rs:37` 已有 `capabilities: Option<Capabilities>,`，
`:110-112` 已做 400 校验，`crates/agent-server/src/http/capabilities/` 整个模块 + ts 导出
（`ts_protocol/export.rs:49`）+ 集成测试（`tests/http_capabilities_declaration.rs`）都在。
**建议**：那一格拆成"声明入口：✅ 061 已落（协议+校验，零装配）／装配：❌ 062 未做"。

**S33 · §九 的"`api.ts` 现在没有 `sendToolResult`"已半数过时**

`:269-272` 说的两件事：`createSession` 已经能带 capabilities 了
（`packages/web/src/api.ts:51` `export async function createSession(capabilities?: Capabilities)`，
065 已落地，`packages/web/src/capabilities/wire.ts` 与 `packages/web/src/mcp/` 都在）；
但 `sendToolResult` 确实**仍然没有**，`packages/web/src/render/tool.ts:17-46` 收到
`tool_executing` 仍只画卡片不执行不回传。那半条真话现在归 066 不归 063。

### `ORCHESTRATION.md` / `INTEGRATION.md`

这两份是 M8/M9 的接缝文档，**设计判断全部经受住了实现**（没有一条危险级）。问题集中在两类。

**S34 · `ORCHESTRATION.md` §四/§五 引用的每一个 `file:line` 都已过期**
（043 加 MCP 路、052/053 搬 spawn 三函数、060 加 remote 路，先后改了那几个文件）：

| 文档 | 原文（节选） | 事实 |
|---|---|---|
| `:57` | `dispatch.rs:70` 的 `Effect::ExecuteTool` 内按工具名截 | `crates/agent-runtime/src/dispatch.rs:87` |
| `:70` | `改 spawn_tool.rs schema + dispatch.rs::spawn` | spawn 三函数已搬回 `spawn_tool.rs`（`:22-26` 说明原因：053 要加第五处截获时 `dispatch.rs` 已贴着红线 9 的 300 行）；bg 路径是 `spawn_tool.rs:281` 的 `detach()` |
| `:77-79` | `subtree::final_text`（`subtree.rs:134`）· `harvest`（`subtree.rs:65-93`） | `final_text` 已拆到 `crates/agent-runtime/src/child_outcome.rs:47`；`harvest` 在 `subtree.rs:200`，收割体 `:206-250` |
| `:83` | `runner.rs` 的 **B 点**，`:140` | 孤儿收尾是独立的 **B0** 点，`crates/agent-runtime/src/runner.rs:154`（`orphan::reap`），在 B（`:161`）**之前** |
| `:45-46` | `Subtree` 局部绑定（`runner.rs:95` 每次 `resume` 重建） | `runner.rs:109`；**结论正确**，行号漂了 |
| `:96-97` | detached 子的结果带 spawn 时的 epoch（`ChildSlot.epoch`，`subtree.rs:45`） | 是**另一个字段** `Detached::epoch`（`subtree.rs:82`）；`ChildSlot::epoch` 在 `:68`，走前台/collect 那条路 |
| `:97` | collect 回写经 `step.rs:69` 的同一道 epoch 门 | `crates/agent-core/src/command/step.rs:71` |

**S35 · `ORCHESTRATION.md:29-30` 引用的那段模块文档已被 052 改写**

文档说 `runner.rs:20-27` 把「root 已终态、子树还在跑」称作**无定义状态并拒绝它**；
`crates/agent-runtime/src/runner.rs:28-33` 现在说：

> **052 的修正**：后台 spawn（`background=true`）让父那个槽在 spawn 那一刻就收敛，于是
> 「root 已经终态、子树还在跑」这个世界**真的存在**了……**原先这段文档说那个世界「没有答案」，是过虑。**

文档自己 §二 的脚注预言过这次修正，只是正文没跟上。

**S36 · `ORCHESTRATION.md:98-100` 的「不新造一套」只对了一半**

文档说 detached 的 epoch 校验"复用异步路已有的门，不新造一套"；
`crates/agent-runtime/src/subtree.rs:264-266, 282` 里 stash 那一步**自己比了一次**：

> 这里必须自己比一次，不是重造机制：真正的门还是那一道……但 stash 这一步**不经过
> `Session::step`**（它不产出任何事件），没有别的地方替它把门。

判据逐字相同，所以设计成立；文档缺这半句会让人以为只有一处门。

**S37 · `INTEGRATION.md:41-42` §三「现状」描述的是 055 之前**

文档说 `POST /sessions` "服务端生成、客户端不能指定"；055 已落地，
`crates/agent-server/src/http/routes/sessions.rs` 的 `CreateSessionRequest` 已有 `id: Option<String>`，
幂等三态按文档表实现，另有文档没提的 `outcome: created|existing|recovered` 响应字段。

**S38 · `INTEGRATION.md:182-183, 200-202` 的 hub 泄漏段整段被 059 取代**

文档还留着"**诚实标注**：这是**静态分析**的结论，**没有实测**"；
`crates/agent-server/src/http/hub/mod.rs:143` 现在只留 `canceller: handle.canceller()`，
`hub/mod.rs:59-61` 记着：

> 这不是推测——`crate::http::state` 的独测（`closing_every_session_empties_the_hub_table`）
> 在修之前稳定复现：三个 session 全部 `close` 之后等五秒，三项一个不少。

**S39 · `INTEGRATION.md:123-125` 的「`agent-server/src` 现在一次都没用过 `tokio::time::timeout`」**

那是写给 056 实现者的前瞻警告；056 已落地，它就是现存代码：
`crates/agent-server/src/http/routes/poll.rs:75` `let _ = tokio::time::timeout(wait, live_rx.recv()).await;`。
另外 `http/state.rs:184` 也有 `sleep`，所以"唯一的定时器是 guard 的 sleep"也不成立。

**S40 · `INTEGRATION.md:173` 的「宽限是兜底、网关正常关闭时应主动发 cancel」漏了一条实现发现的约束**

058 落地时发现：同一个 chatid 上多个浏览器 tab 共享一个 Rust session，关一个 tab 就发 cancel
会取消整个会话。参考实现因此加了一份网关侧的观众计数
（`examples/java-gateway/src/main/java/com/example/agentgateway/proxy/ChatSubscribers.java`），
`AgentSseController.java:46-59`：

> 本网关在这个 chatid 上还有别的连接就什么都不做——Rust 的引用计数还没归零，
> 取消会误伤别的 tab。真的一个不剩才发 cancel

**这条值得补进文档**，它是拷走这份参考实现的人最容易漏掉的一处。

---

## 三、小瑕疵（19 条）

| # | 文档位置 | 原文 | 事实 | 建议 |
|---|---|---|---|---|
| N1 | `INVARIANTS.md:24` | hook 粗筛 **`agent-core/src/atoms/`** 下的 `Instant::now` / … | 那个目录不存在。脚本查 `crates/agent-core/src/atoms/*` **和** `crates/agent-core/src/graph/*`，且自己注了原因（`scripts/check-invariants.sh:145-149`：026 把构图函数放在 `graph/`，"`atoms/` 保留在名单里：M3 若长出那个目录，不必再改一次脚本"） | 改成 `graph/` |
| N2 | `INVARIANTS.md:132` | 检查：hook grep `agent-server` 下硬编码的 `0.0.0.0` | 脚本给的是**警告**不是违规（`scripts/check-invariants.sh:190` 用 `w` 不是 `v`）。红线 9 那条写了等级，这条没写 | 补一句"警告级" |
| N3 | `ROADMAP.md:44` | `crates/` **六个 crate**（M1 产物，见下） | 十个（S2）。那个树也没列 `apps/` `packages/` `examples/` | 同 S2 |
| N4 | `docs/issues/` | — | **M10 的 issue 有四组重号**（见 D9），另加 `038-frontend-tools.md` ↔ `038-skill-injection-probe.md` | 见 D9。**该目录正被并发修改** |
| N5 | `ARCHITECTURE.md:52` | `agent-server-bin/   二十行的 main.rs` | **正确**（`crates/agent-server-bin/src/main.rs` 恰好 20 行），但同目录还有 `cli.rs`(197)/`ready_file.rs`(193)/`run.rs`(175)，"二十行"易被读成整个 crate | 加半句"（装配在 `run.rs`/`cli.rs`）" |
| N6 | `MCP.md:160` | 活句柄进 atom/快照 \| 违反红线 3（**编译期挡不住**，review 挡） | 挡得住：`crates/agent-mcp/src/registry.rs:3-6` "`agent-core` 压根不依赖 `agent-mcp`……类型层面就够不着 `McpClient`/`McpRegistry`——**不是「没写进去」而是「写不进去」**。结构性证明见 `tests/registry_not_in_snapshot_042.rs`" | 改成"由依赖方向结构性挡住"，review 那句留给未来同时依赖两边的 crate |
| N7 | `MCP.md:32` | 只有这三样。**没有第四样** | `crates/agent-mcp/src/tool_result.rs:13-16` 的 `ToolCallOutput { text, is_error }` 也过缝（`mcp_call.rs:32,75-77`）。文档的**意图**（wire 类型不过缝）仍成立 | ASCII 表加一行 `tools/call 结果 ──→ 拍平 ──→ ToolCallOutput（043）`，"没有第四样"改成"wire 类型一样都不过缝" |
| N8 | `OBSERVABILITY.md:65-66` | `Working` 带在飞的工具名 / **在等的子 agent** | `crates/agent-core/src/observe.rs:111-117` 只收工具名；等子 agent 表现为字符串 `"srv:agent/spawn"`，不带子的 id | 说清"等子 agent 表现为工具名本身，子的形状从树的其余节点看" |
| N9 | `ORCHESTRATION.md:62` | `AgentActivity`（Idle/Thinking/Working{tools}/**Done**/Failed） | `crates/agent-core/src/observe.rs:55` 是 `Done { truncated: bool }`，模型看到 `Done` / `Done(truncated)`（`status_tool.rs:201-202`）。而 `status_tool.rs:191-192` 声称这些词"跟 §三那张表逐字对得上" | 表里补 `Done(truncated)` 与 `Failed(原因)` |
| N10 | `ORCHESTRATION.md:72-73` | `Event::ToolResult{agent:parent, call_id, content:"{agent_id}"}` | 少 `epoch`，且 content 是 JSON：`crates/agent-runtime/src/spawn_tool.rs:288` `json!({ "agent_id": child.as_str() }).to_string()`；`:298` 带 `epoch` | 补 `epoch`，content 写成 `{"agent_id":"<id>"}` |
| N11 | `ORCHESTRATION.md:75-77` | `subtree.record(id, parent, collect_call_id, epoch)` | 五参：`crates/agent-runtime/src/subtree.rs:113-120` 多一个 `tool: &'static str`（053 加的，好让 `ToolExecuted` 报 `collect` 而不是 `spawn`） | 补第五参 |
| N12 | `INTEGRATION.md:126-127` | 无 `Last-Event-ID` → …**必然是 Backlog**、永不触发 Gap | 空 ring 时是 `Live`：`crates/agent-server/src/http/hub/ring.rs:88` `(None, None) => return Replay::Live`。承重的那一半（永不触发 Gap）正确 | `必然是 Backlog（缓冲区空时是 Live）` |
| N13 | `INTEGRATION.md:209-211` | Java 片段 `new ProcessBuilder(binPath, "--port","0", "--sessions-dir",dir, "--ready-file",…)` | 真实实现还必须传 `--config`（`examples/.../runtime/AgentServerProcess.java:51-56`），key 走环境变量（`:61`）。照片段起的子进程没有 provider 配置 | 补 `--config`，或加一句"完整版见 `AgentServerProcess.java`" |
| N14 | `INTEGRATION.md:33-35` | 四个坑一次性消失……**MVC 也能扛** | 不是矛盾（"MVC *可以* 做到"），但参考实现仍是 WebFlux（`examples/java-gateway/pom.xml:44`），从 §一「企业存量大多是 MVC」读过来的人会期待一个 MVC 样例 | 加一句指向 `examples/java-gateway/README.md` §Why this remains WebFlux |
| N15 | `HOST-CAPABILITIES.md:39` | 这条同时决定了 **§五** 的排序账怎么算 | 排序/红线 11 是 `## 六`（`:98`），`## 五` 是可逆性（`:84`） | `§五` → `§六` |
| N16 | `HOST-CAPABILITIES.md:66` | `"reversibility": "pure" }          // 可选，见 §四` | 可逆性在 `## 五`（`:84`）；这条注释本身就在 §四 里，指向了自己 | `见 §四` → `见 §五` |
| N17 | `HOST-CAPABILITIES.md:278` | **安全相关的 issue** 待 **§七** 定稿后补 | §七 是 MCP（`:110`），安全是第二个 §八（`:170`）；上一行 `:277` 已经写着"安全（§八）暂缓" | 改成安全那节的编号（配合 S31 的重编号） |
| N18 | `HOST-CAPABILITIES.md:15` | 执行 \| ✅ **完整且测过**。`Location::Web`/`Desktop` → … | Desktop 半边没有任何注册工具、没有端到端测试（S8）；端到端测试只有 Web（`crates/agent-server/tests/web_tool_result_resumes_turn.rs:35`、`web_tool_never_answered_times_out.rs:47`） | "Web 侧完整且测过；Desktop 走同一条 `is_remote` 路，暂无注册工具/测试" |
| N19 | `HOST-CAPABILITIES.md:24-26` / `:149` | 位置和可逆性靠两个**不查表的自由函数**按名字推 · `location_of` 那块**正被 [050] 拍**（别撞） | ① `mcp:` 的可逆性**是查表的**（`tool_table.rs:199-203`），`location_of` 另有三个无前缀历史别名（`:214`）；② `docs/issues/050-tool-name-encoding.md:3` 标着"待归类"，`grep -n "050" docs/issues/README.md` **零命中**——它未排期，不是在做 | ① 补"`mcp:` 例外，查 `ToolTable::mcp_reversibility`"；② 改成"050 仍未排期，改 `location_of` 前先看它" |

`POST /tool_result` 这个简写路径在 `HOST-CAPABILITIES.md:15,134,157,262`、`TOOLS.md:38`、
`ARCHITECTURE.md:122` 都出现（全路径是 `/sessions/{id}/tool_result`，
`crates/agent-server/src/http/routes/mod.rs:27`）。对人无害，对生成代码的 agent 有害——
建议每份文档至少写一次全路径。

---

## 四、⚠️ 疑似代码问题（不是文档问题）

### A · 【确定的 bug】所有 MCP 调用被一把全局锁串行化，锁还跨了整个 JSON-RPC 往返

**文档承诺** `docs/MCP.md:78`

> | 异步 | provider 调用（`provider_call::start/finish`，起 IO 线程 + 在飞凭据 + 泵管落地） | **多 agent 并行不被掐死**；epoch 校验天然在这条路上 |

`docs/MCP.md:82`

> 1. **MCP 的慢没有上限**——阻塞 actor 线程不可接受（一个 agent 等 server，全树停摆）。

**代码** `crates/agent-mcp/src/registry.rs:53-56`

```rust
    pub fn with_client<T>(&self, server_id: &str, f: impl FnOnce(&mut McpClient) -> T) -> Option<T> {
        let mut clients = self.clients.lock().unwrap();
        clients.get_mut(server_id).map(f)
    }
```

**代码** `crates/agent-runtime/src/mcp_call.rs:72-73`

```rust
        let outcome =
            registry.with_client(&server_id, |client| client.call(&bare, arguments, timeout));
```

`clients` 是**一个** `Mutex<HashMap<String, McpClient>>`（`registry.rs:22`）。闭包 `f` 在**锁内**
执行，而 `McpClient::call` 是阻塞式 JSON-RPC 往返——于是**调 server `a` 会把并发调 server `b`
挡住最多一整个 `ctx.mcp_timeout`**（默认 `DEFAULT_CALL_TIMEOUT = 30s`，`client.rs:48`）。

**代码自己承认这本该在 043 修掉，而 043 发了没修** `crates/agent-mcp/src/registry.rs:16-19`

> 锁只在单次操作期间持住——`McpClient::call`/`list_tools` 是阻塞式往返（042 范围），
> 持锁期间调用方会等一整个 JSON-RPC round trip，**这是暂时的：043 的异步执行路会把
> 「发请求」和「等响应」拆开，不再需要在锁里跨一次完整往返。**

**为什么现在是活的问题而不是理论问题**：M8（052/053）让多个后台子 agent 真的并发跑，
而 `McpCall` 明确支持多张在飞——`crates/agent-runtime/src/mcp_call.rs:39-41`：
`/// 一次在飞的 MCP 调用。**一个 agent 可以同时有多张**`。于是 N 个并行 MCP 工具调用
严格串行执行。

**影响范围**：actor 线程**确实**没被阻塞（这半个承诺成立），但"多 agent 并行不被掐死"
对 MCP 不成立；**一个挂住的 MCP server 会拖慢进程内所有 MCP 调用**。
次要问题：`.lock().unwrap()` 意味着任何一个 client 的 panic 会毒化整张 registry。

**建议（未实施）**：把 client 从 map 里搬出来——`HashMap<String, Arc<Mutex<McpClient>>>`，
`with_client` 改成只在查表时持全局锁、返回一个 per-server 句柄，`mcp_call::start` 只锁那个句柄。
或者兑现 `registry.rs:16-19` 那条注释，把发/收拆开。**无论选哪个，那段注释都该同步改**——
它现在承诺了一个没兑现的修复。**建议单开 issue。**

### B · `srv:agent/status` 的模型可见描述，对后台子 agent 说了假话

**代码** `crates/agent-runtime/src/status_tool.rs:68-69`（`status_spec().description` 的一段）

> 它**不返回子 agent 的回答正文**——正文会在那次 spawn 调用的结果里回到你这里。

**但对 `background=true` 的子，spawn 调用的结果里没有正文**——
`crates/agent-runtime/src/spawn_tool.rs:288`：

```rust
    let content = json!({ "agent_id": child.as_str() }).to_string();
```

正文只能经 collect 拿到（`crates/agent-runtime/src/collect_tool.rs:58-59`）。同一个文件的**模块**
文档（`status_tool.rs:12`）说的是对的——"正文是 `collect` 的事（053），走另一条路"——
和喂给模型的那句话自相矛盾。这是 051 写的文案，052/053 没回来改。

**为什么值得单列 issue 而不是当排版问题**：
① 这段字符串进**每一个**开了 `with_status()` 的会话的**每一轮** prompt（`ToolTableSpec::Full`、
`agent-cli` 都开），是红线 11 的辖区；
② 失败模式正是本仓点名的"不报错只在别处浮出来"那一类——模型读到"正文会从 spawn 回来"，
判断不需要 collect，轮末后台子被拆掉、结果丢弃（`crates/agent-runtime/src/orphan.rs` 发
`OrphanFate::Discarded`）。**测试不会红**，因为没有任何测试断言工具描述的文本内容。

**减轻因素**：`collect_spec()` 的描述里有一句反向提醒——"**你这一轮结束前没领的后台子 agent
会被拆掉、结果丢弃**——开了后台就记得回来领"。两段话打架，模型未必被带偏。所以这是
**可用性缺陷**，不是数据事故。

**建议（未实施）**：`status_tool.rs:68-69` 改成区分两种 spawn；加一条回归断言：
`status_spec().description` 必须提到 `collect`。

### C · 崩溃恢复时"在飞的 tool call 按可逆性分三种处理"没有落盘依据

见 S17。`docs/STATE-MODEL.md:271-280` 的中断语义表需要"发起当时的 `Reversibility`"，
而那份快照（`ToolCallSlot::Request`）没有槽位、不落盘，只活在宿主内存
（`crates/agent-runtime/src/ctx_remote_tools.rs:55`）。

**这更像"已知的未实现"而不是 bug**：`crates/agent-core/src/graph/slot.rs:91-94` 明确记录了
为什么先不做（"core 没有工具表，现造一份占位快照是编造——假的 `Irreversible` 会让 undo
白拦一次 `fs/read`，正是静默错值"），这个判断是对的。

**但要确认的是**：崩溃恢复路径现在对"崩溃时有 tool call 在飞"做了什么。
`crates/agent-core/src/command/restore.rs` 只灌快照 + 推 entries；一个持 `Pending` 的
`ToolCall(_, _, Result)` atom 恢复后仍然是 `Pending`，而执行现场（`PendingRemoteTool`、
provider 的 HTTP stream、MCP 子进程）已随进程消失。**这个 session 会不会永远卡在 `ToolsPending`？**
本次审计没有验证这条路径（需要真跑 kill -9 + 在飞工具的组合）。

**建议（未实施）**：单开 issue 验证"kill -9 在 tool call 在飞时"的恢复行为，并按结果决定是补
`ToolCallSlot::Request`，还是在恢复时把未收敛的槽位统一按 turn 粒度抹掉——
`STATE-MODEL.md:282` 其实已经给了后一种答案（"未完成的 turn 用 turn 粒度的 undo 直接抹掉"），
如果代码已经这么做了，那 §中断语义 上半张表就是**多余的设计，该删而不是该实现**。

### D · 给 062 的地雷：`ToolTable::snapshot` 会静默丢弃宿主声明的 `reversibility`

`crates/agent-runtime/src/tool_table.rs:199-203` 的查表分支门是 `tool.starts_with("mcp:")`。
一个声明了 `"reversibility": "pure"` 的 `web:crm/lookup`（`HOST-CAPABILITIES.md:86` 说
"→ **就用它**"）会落进 `reversibility_of(tool)` → `_ => Reversibility::Irreversible`。

今天不是 bug（还没有装配路径），但 062 若只加第二张 `BTreeMap` 而不拓宽那个分支门，
§五 的承诺就会**朝安全方向静默失效**——没有测试会红，只是 `/undo` 会在宿主说是纯读的工具上
停下来问。**建议**：在 062 的 issue 里显式写一行。

---

## 五、核对过并确认**正确**的（摘要）

避免"没提到 = 没查"的误读。

**红线 12 条全部逐条核对，规则本身全部成立**：红线 1（derived 纯函数）、2（`store.set` 白名单）、
3（`AgentValue` 不提供 `dyn Any`）、4（落盘用 `AtomKey` + 孪生条款，`graph/slot.rs:101-109`）、
5（大值 `Arc` + `imbl::Vector`）、6（epoch 门在 `crates/agent-core/src/command/step.rs:71`；
另一处 stash 专用门在 `subtree.rs:282`，判据逐字相同）、7（两个 crate 无 IO 依赖）、
8（`crates/agent-server/src/bind.rs:58-59` 用 `Ipv4Addr::LOCALHOST`，源码里刻意不出现那个字面量）、
9（行数）、10（不相交集合 + 穷举 match 无 `_` 通配 + 集合性质测试，`graph/visibility.rs`）、
11（`ToolSpec` 的 `serde_json::Value` 后端是 `BTreeMap`，根 `Cargo.toml:37-40` 刻意不开
`preserve_order`，`value/tool.rs:146-194` 有逐字节测试）、12（`agent-core` 无厂商名/能力位）。
只有举例槽位名（S18）、检查等级（N2）、检查路径（N1）三处细节过期。
`bash scripts/check-invariants.sh --all` 当前**通过**。

**ARCHITECTURE**：`AgentServer::new(config).serve(addr)` 是唯一入口（`crates/agent-server/src/lib.rs:36`
明确引用本文档）· `main.rs` 二十行 · 两个 SSE header 都发且 `routes/sse.rs:21-23` 引用本节作为依据 ·
`agent-server` 是库不是二进制 · `agent-store` 不认识 agent/消息/工具 · 协议类型由 Rust 生成且
本地收工测试卡一致性 · `probes/api` 与 `apps/desktop/src-tauri` 是独立 workspace（`Cargo.toml:16-21`）。

**ADAPTER**：`Provider` trait 四个方法逐字对上（`crates/agent-providers/src/lib.rs:90-105`）·
`Encoded` 五字段全对 · `Decoded` 三字段 · `Ingredients: Send` 结构性挡住 store 句柄 ·
`Adjustment`/`ErrorClass`/`PrefixImage` 定义在 `agent-core` · `intent: RequestIntent` 而非
`tool_choice` · adapter 整层零 IO。

**TOOLS §reversibility / §MCP**：三级判据与"拿不准就 `Irreversible`"逐字兑现
（`value/tool.rs:43-49`、`tool_table.rs:268`）· `Irreversible` 挡 undo、`Reversible` 不挡 ·
只有 `Pure` 可重放 · `readOnlyHint == Some(true)` → `Pure`、其余一律 `Irreversible`
（`agent-mcp/src/translate.rs:38-39`，`:87-98` 四取值穷举测试）· "registry 要能表达这个源在这个
host 上不可用"已落地（`availability.rs` 的 `Host` × `TransportKind` 门）· MCP 可逆性 per-tool 查表、
查不到落保守值 · MCP 调用是 `Location::Server`。

**STATE-MODEL**：`AtomKey` 只有两个变体、没有 `Skill(SkillId)` · `DerivedKey` 刻意不 derive serde ·
`imbl::Vector` 存消息历史 · `turn_id` 由 root 分配、子继承 · 恢复 = redo 同一个函数 ·
逐出/重建三条硬约束 · epoch 恢复取"见过的最大值 + 1"及其论证。

**MCP**：crate 一分为二（041 纯协议 / 042 IO）· 三样东西过缝 · `agent-core` grep 不到 mcp/jsonrpc ·
异步执行三段式 `mcp_call::start/finish/take` 复用 provider 的在飞账本 · M6 只做 stdio ·
`.mcp.json` / `mcpServers` / server id 去重是硬错误 · 失败隔离 · 名字经 050 的 wire 编码层
仍能往返（`tool_name.rs:88-95`）· CLI `/mcp` 存在。

**OBSERVABILITY**：树由 core 权威算（`observe.rs:79-82`）· CLI 与 web 共用同一个 `agent_tree()` ·
纯派生读、undo 后子从树上消失有回归测试（`observe.rs:209-222`）· `AgentNode` 五个字段 ·
不为"当前动作"加 primitive · `usage` 不在 `AgentNode` 里 · SSE 事件名就叫 `agent_tree` ·
`GET /sessions/{id}/agents` 做种端点存在。

**ORCHESTRATION**：三个工具名 · `background` 默认 false · 三者可逆性判定 · `declares` 把关的
五处截获 · status 非阻塞 · collect 留 `Pending` · 孤儿收尾是 per-child `despawn_child` 而非
会话级 cancel · 红线 10/11 在 status 路径上的落点——全部核对通过。

**INTEGRATION**：`GET /events/poll` 形状 · 游标复用 `Last-Event-ID` header（axum 确实没开
`query` feature：`crates/agent-server/Cargo.toml:28-30`）· `X-Poll-Wait-Ms` 语义 ·
响应 `{frames,next}` · `next` 服务端算且空批保持原游标 · ring 默认 256 / id 从 1 起 / 0 保留 ·
`Replay` 三变体与 `Gap.skipped` 精确 · per-poll `SubscriberGuard` 跨 await 持有 ·
5s 取消宽限 · SSE 与 poll 共享同一个计数器 · id 白名单 `[A-Za-z0-9_-]` ≤128 拒绝不 sanitize ·
`--port 0` / `--sessions-dir` / `--ready-file` · ready-file 用 **hard_link 而非 rename** ·
Java 侧校验 pid · SIGTERM 优雅落盘——全部核对通过。

**HOST-CAPABILITIES**：§一 对 `ToolDescriptor` 债务的记述是全仓唯一正确的 · remote tool 执行链
全对 · skill 延迟加载形状全对 · `ToolTableSpec` 确实五档且 `build()` 从不调 `.with_skills`/`.with_mcp`
（§八「server 形态下 skill 从没装载过」**属实**）· `OpenSpec.tools` 确实 per-session ·
`mcp_reversibility` 先例 · §七 否决形态 A 的论据成立（`agent-mcp` 只有 stdio）·
§四 的协议形状与落地的 061 逐字节一致 · 安全节引用的事实（红线 8、ClusterIP、chatid 白名单、
三态 outcome、32 KiB 工具结果上限）全部核对通过 · 所有相对链接可解析。

---

## 六、给修文档的人的三条建议

1. **先修 §零 那三条 + D9/D10**。前三条是"照着写就会错"，D9 是"照着派活就会派错"，
   D10 是"再不拍就永久固化"。其余按 issue 顺手带。
2. **`ORCHESTRATION.md` 的 `file:line` 引用全部换成符号名。** 这份文档里的行号已经错了七处，
   在这个仓的演进速度下行号引用是负资产。同理适用于任何新写的接缝文档。
3. **凡是代码注释里已经写明"文档说 X，实际是 Y"的地方，让文档去追代码。**
   本次审计里 `graph/slot.rs:20-24`、`graph/visibility.rs:39-43`、`registry/mod.rs:2-5`、
   `history/cap.rs:9-13`、`runner.rs:28-33`、`agent-mcp/src/registry.rs:3-6`、
   `agent-server/src/bootstrap.rs:124` 七处都是代码比文档诚实——这些是最容易改、
   收益最确定的。
   **例外是 `agent-mcp/src/registry.rs:16-19` 和 `config.rs:46-47`**：那两处是代码注释自己在
   说假话（承诺了没兑现的修复、声称了不存在的落盘），要反过来改代码注释。
