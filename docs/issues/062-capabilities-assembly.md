# 062 per-session 装配：注入的工具真的进这个会话的表

**里程碑** M10 · **依赖** 061 · **模型** opus · **独测** ✅（063 与本 issue **并行**写）

把 061 校验好的声明**装进这一个会话的工具表**。碰 `OpenSpec`/`ToolTable`，是 M10 的结构性
一块。接缝见 [HOST-CAPABILITIES.md](../HOST-CAPABILITIES.md) §二/§五。

## ⚠️ 声明从哪来（2026-08-04 用户拍板 · 最终版，推翻本节此前两版表述）

**注入的声明是会话状态，不是部署配置**：正确的归宿是建会话时**写进 store**（journaled，
进 undo log），恢复时跟别的状态一样**从日志回放自动回来**，宿主**不必也不该**在重连时
再声明一遍。理由三条：

1. 历史对话是在**那一份**工具表下产生的，恢复时换成前端今天的新清单，历史就自相矛盾
   （模型当初说「我调了 `web:crm/lookup`」，而新清单里可能没有它了）；
2. 红线 11——工具表在 prompt 最前面，换一份 = 恢复出来的第一轮前缀全断，而它本该接着
   用缓存；
3. 本仓的核心是「undo / 恢复 / 审计是同一套机制的投影」，**恢复是忠实重放，不是用今天
   的配置重建**。这跟 skill 的既有模式同构：激活状态在 store，内容在运行时 registry。

**对 062 的影响：范围不变，只有「声明从哪来」变了。** 本 issue 做的是「拿到一份声明 →
装配进这个会话的表」，那部分照旧全部有用；把声明搬进 store 是 **073**（碰 store/command
层，红线 3，opus 单独做）。062 因此：

- **不碰 store**，但**接口留对**：装配那一步的入参是「一份声明」（纯 `agent_core` 数据），
  **不跟「这次 HTTP 请求」耦死**。073 落地后装配这一侧一行都不用改。
- `existing`（会话还活在 registry 里）**忽略**这次的声明——中途换表 = 前缀全断（红线 11），
  那是「运行时增删」，接缝 §三 明确不做。
- **不写「关掉再开、带同样声明、断言工具还在」那条断言**（本节早先版本要求过，已撤回）：
  073 之后正确的行为是**不带声明也该还在**（从日志回来），钉住它等于把「宿主必须重连时
  重报一遍」写成契约。中间态如实记在实做记录里。

## 范围

1. **作用域 = 这个 chatid 的会话**（接缝 §二，**本 issue 最重要的一条**）：注入的工具
   **不进全局表**、不影响别的会话、会话结束就没了。
   **路径已勘查清楚**：`OpenSpec.tools` 本来就是 per-session 字段（`SessionRegistry::open`
   收的就是它，测试里早就 per-session 地改），挡路的只有
   `SessionTemplate::open_spec` 无条件抄 `self.tools` 那一行。
   **让 `open_spec` 接受这次请求带来的注入部分——不要给全局表开写口。**
2. **`ToolTable` 追加注入的 `ToolSpec`**：追加在**表尾**（连 MCP 之后），
   前面那段所有会话共有的字节一个都不动。
3. **可逆性映射**：另挂一张 `BTreeMap<Arc<str>, Reversibility>`，照 `mcp_reversibility`
   的既有先例；**不动 `ToolSpec` 的三字段形状**（它进 prompt，加字段要重算红线 11 的账，
   而可逆性纯查表、不进 prompt）。声明了就用，**没声明落保守 `Irreversible`**。
4. **`snapshot()` 的判定**：注入工具的 `location` 走既有 `location_of`（`web:` → `Web`），
   `reversibility` 先查注入映射、再查 MCP 映射、最后落既有规则。三张表的优先级要写死并有测试。

## 验收（可判定）

- 声明 `web:crm/lookup` → **该会话**的 `specs()` 里有它，且 `declares("web:crm/lookup")` 为真。
- **作用域隔离（必测）**：同一个 server 上另起一个**不带声明**的会话 →
  它的表里**没有**这个工具，`declares` 为假。
- `reversibility: "pure"` → `snapshot()` 给 `Pure`；不声明 → `Irreversible`；`location` 恒 `Web`。
- 注入的工具**排在表尾**：拿一个不带声明的会话的 `specs()` 做基线，
  带声明那个的**前 N 项与基线逐项相同**。
- 不带 `capabilities` 的会话：工具表与本 issue 之前**逐字节相同**（既有测试全绿）。

## 注意

- **红线 11 的字节确定性锁在 [063](063-capabilities-determinism.md)**，与本 issue **并行**
  进行（本仓「接口先定 → 实现与测试并行 → 合并」的既有模式，043 的注意里写过）。
  你**仍要**在实现里做「内部按名字排序」，063 负责把它钉死到会红的程度。
- **执行不在本 issue**：注入工具怎么跑走的是既有 remote 通道（060 修完后是安全的）。
  本 issue 只管「进得来、进对地方」。
- **不要碰** `crates/agent-tools/`；`location_of` 别动（050 的地盘）。
- **撞名不归本 issue，但别在这里随手定死它**（[069](069-name-collision-policy.md) 已拍板）：
  `with_host_tools` 撞上表里已有的名字时该「**后来的整条不进表**」（spec 不 push、可逆性
  也不 insert），**不是 panic**、更不是留两份。今天实测五档 + CLI 链**没有任何撞名**，
  且没有内置工具用 `web:`/`desk:` 前缀（`agent-runtime/tests/tool_table_names_are_unique.rs`
  钉住了这两条），所以**本 issue 不需要为它写一行代码**；查重的实现排在本 issue **之后**，
  免得跟你的可逆性映射重构撞车。跨路径那一半（`late_tools`）在 064。
- 红线 9：`tool_table.rs` 现在 286 行，**只剩 14 行余量**——加映射表大概率要拆，
  按职责拆（先例：`#[cfg(test)] mod tests` 挪进 `#[path]` 子文件）。
- 收工验证前台跑完（WORKFLOW §四 -1）。

## 实做记录（完成 · 2026-08-04）

### 装配路径：一条直线，四个文件各管一段

```
POST /sessions  {"capabilities":{"tools":[…]}}
  │
  ├─ capabilities::validate            061 的纯函数，名字不合规 → 400（会话不被创建）
  ├─ capabilities::host_tools          062 新增：声明 → Vec<(ToolSpec, Reversibility)>
  │                                    「没说 reversibility」在这里落成保守 Irreversible
  ├─ SessionTemplate::open_spec(id, session_path, host_tools)   ← 新签名，第三个参数
  │                                    只落进这一次的 OpenSpec；self 一个字节不动
  ├─ SessionRegistry::open(spec) → actor::spawn → 独立线程
  └─ actor::body::run:  spec.tools.build().with_host_tools(spec.host_tools)
                        ↑ 部署期五档（所有会话逐字节相同）  ↑ 这个会话专属的表尾
```

| 文件 | 行 | 干什么 |
|---|---|---|
| `crates/agent-runtime/src/tool_table_host.rs` | 70 | **新**：`ToolTable::with_host_tools`——排序 + 追加表尾 + 记可逆性映射 |
| `crates/agent-runtime/src/tool_table_host_tests.rs` | 134 | **新**：6 条单测（表尾、排序、三级优先级、location、空注入） |
| `crates/agent-runtime/src/tool_table.rs` | 296 | 加 `host_reversibility` 字段 + `snapshot` 的三级判定（286 → 296） |
| `crates/agent-server/src/http/capabilities/assemble.rs` | 133 | **新**：声明 → `(ToolSpec, Reversibility)`，含「没说落保守」+ 4 条单测 |
| `crates/agent-server/src/http/capabilities/mod.rs` | 226 | 挂 `assemble`、删 061 留的 `#[allow(dead_code)]`（见下） |
| `crates/agent-server/src/registry/spec.rs` | 170 | `OpenSpec` 加 `host_tools` |
| `crates/agent-server/src/http/config.rs` | 266 | `open_spec` 新签名 + 一条「注入不粘在 template 上」的单测 |
| `crates/agent-server/src/http/routes/sessions.rs` | 219 | 校验之后翻译、当参数传下去（187 → 219，主要是文档） |
| `crates/agent-server/src/actor/body.rs` | 228 | 一行：`.with_host_tools(spec.host_tools)` |
| `crates/agent-server/tests/http_capabilities_scoped_to_one_session.rs` | 123 | **新**：端到端作用域隔离（断言落在假上游收到的请求体上） |

`tool_table.rs` 按红线 9 拆过：拆出去的是**「宿主注入这件事」**（那张映射怎么进表、
为什么排序、为什么另挂表、查表的门为什么按表不按前缀），不是「后半截代码」。工具表
的五档装配、名字规则、`snapshot` 的另外两级留在原处；`location_of` 一个字节没动
（050 地盘）。

### 三张可逆性表的优先级（写死在 `snapshot`，有测试）

```rust
let reversibility = match self.host_reversibility.get(tool).copied() {
    Some(declared) => declared,                                  // ① 宿主注入
    None if tool.starts_with("mcp:") => self.mcp_reversibility    // ② MCP（042）
        .get(tool).copied().unwrap_or(Reversibility::Irreversible),
    None => reversibility_of(tool),                               // ③ 名字规则（既有）
};
```

**第一级按「表」查，不按前缀查**——这是本 issue 最容易踩错的一格。062 之前这里只有
一个 `if tool.starts_with("mcp:")` 的**前缀门**，照抄它给注入表配一个 `web:`/`desk:`
前缀门看着等价，实际上把「谁被注入了」和「名字长什么样」耦死：注入的工具一旦落回
`reversibility_of` 的 `_ => Irreversible` 兜底，症状是**宿主声明了 `pure` 却按
`Irreversible` 办——功能「正常」、一声不吭**，只有 `/undo` 撞上去停下来问才看得出来。
按表查没有这个耦合。两条优先级各有一条测试（`the_injection_map_wins_over_the_name_rules`
/ `the_injection_map_wins_over_the_mcp_map`），后者故意造一个 HTTP 层不可能出现的状态
（同名同时在两张表里，`mcp:` 前缀会被 061 的校验拒掉）——那条测试的用处就是把优先级
本身钉住，反过来就红。

「没声明 → `Irreversible`」落在 **`assemble`**（`agent-server`）而不是 `ToolTable`：
`with_host_tools` 收到什么记什么、不替调用方猜；协议层（061）如实记录「说没说」，
装配层做解释。三层各一句话，没有一层重复解释。

### 作用域隔离是怎么断言的：断在假上游收到的请求体上

`tests/http_capabilities_scoped_to_one_session.rs` 在**同一个 `AgentServer`、同一份
`SessionTemplate`** 上开三个 chatid，各发一句话，然后去翻假上游收到的三次 provider
请求体里的 `tools` 数组（工具表最终的用处就是变成 prompt 字节，断在这里才算真的证到；
名字在 wire 上是转义过的，用 `agent_providers::wire_name::from_wire` 还原，050 的规则
不在测试里抄第二遍）：

| chatid | 声明 | 断言 |
|---|---|---|
| `plain` | 无 | `tools` 恰好是 `["srv:fs/read","srv:fs/list"]`，**没有任何 `web:`/`desk:` 名字** |
| `declared` | 两个工具（**故意按 `web:`、`desk:` 的乱序给**） | 前 N 项与 `plain` **逐项相同（整个 JSON 对象，不只名字）**；表尾是 `["desk:clipboard/write","web:crm/lookup"]`；描述与 schema 真的进了 prompt |
| `shuffled` | 同两个工具、又换一种顺序 | `tools` 段与 `declared` **逐字节相同** |

**三条反向验证**（每条都真的跑红过，不是「反正它绿」）：

```
① 去掉 body.rs 的 .with_host_tools(…)：
   assertion failed: 注入的排表尾、按名字排序
     left: []   right: ["desk:clipboard/write", "web:crm/lookup"]

② 去掉 with_host_tools 里的 sort_by（红线 11 第二条）：
   e2e:  left: ["web:crm/lookup", "desk:clipboard/write"]  right: ["desk:clipboard/write", "web:crm/lookup"]
   单测: tool_table::host::tests::the_client_array_order_never_reaches_the_table … FAILED
         tool_table::host::tests::injected_tools_are_appended_after_everything_the_sessions_share … FAILED
```

（第三条见下一节。）「不带 `capabilities` 的会话逐字节不变」由 `plain` 那一列 + 061
的四条 + 既有 85 个测试二进制全绿共同守着：`host_tools` 为空时 `with_host_tools` 是
空操作，那条路上一个分支都没多。

### 与 060 的合并点：照 `remote_tool_timeout` 的形状，一个字段都没丢

060 刚在 `SessionTemplate` / `OpenSpec` / `open_spec` 各加了一个 `remote_tool_timeout`
透传。062 碰的是同样这三处，但**两者形状不同，不能照抄**：

- `remote_tool_timeout` 是**部署配置**——`SessionTemplate` 的字段，`open_spec` 从
  `self` 抄进 `OpenSpec`。
- `host_tools` 是**这一次请求带来的**——所以它是 `open_spec` 的**参数**（跟 `id`/
  `session_path` 一档），`SessionTemplate` 上**没有**这个字段。这正是 issue §范围
  第 1 条要的：「让 `open_spec` 接受这次请求带来的注入部分——不要给全局表开写口」。
  `SessionTemplate` 全进程只有一份（`AppState` 持有），往它身上写就等于 A 客户端声明
  的工具 B 客户端下次建会话也看得见。`config.rs` 里那条
  `injected_tools_ride_this_one_spec_and_never_stick_to_the_template` 就是守这个的。

060 的 `web_tool_never_answered_times_out.rs` 与 `web_tool_result_resumes_turn.rs` 全绿
（见下面验证输出），`remote_tool_timeout` 在 `SessionTemplate`/`OpenSpec`/`open_spec`
三处原样保留。

### 061 留的 `#[allow(dead_code)]`：删掉一半，另一半收窄到字段上

`CapabilityTool` 上那句整个删掉——四个字段现在全被 `assemble::host_tools` 读。
`CapabilitySkill` 的 `description`/`body` 还没有读者（**skill 装配是 064**），所以那半
留着，但从结构体级收窄到**字段级**：`id`/`tools` 已经有读者（校验，以及 064 要接的那
一半），别让一句结构体级的 allow 把它们将来真的没人读也一并盖住。

### 声明的来源：062 只做「拿到一份声明 → 装配」，073 换来源

用户拍板的语义：**注入的声明是会话状态，不是部署配置**——正确的归宿是建会话时写进
store（journaled），恢复时跟别的状态一样从日志回放自动回来，宿主**不必也不该**在重连
时再声明一遍。理由三条：①历史对话是在**那一份**工具表下产生的，用今天的新清单重建就
自相矛盾（模型当初说「我调了 `web:crm/lookup`」，而今天的清单里可能没有它）；②红线
11——换一份表 = 恢复出来的第一轮前缀全断；③本仓「undo/恢复/审计是同一套机制的投影」，
**恢复是忠实重放，不是用今天的配置重建**。这跟 skill 的既有模式同构：激活状态在 store，
内容在运行时 registry。

那一步是 **073**（碰 store/command 层，红线 3，要 opus 单独做）。**062 不碰 store**，
但把接口留对了：装配这一侧的入参是**「一份声明」而不是「一次 HTTP 请求」**——
`assemble::host_tools` 吃已经解析好的 `Capabilities`、吐纯 `agent_core` 数据，
`open_spec`/`OpenSpec.host_tools` 收的是 `Vec<(ToolSpec, Reversibility)>`，两头都不认识
`CreateSessionRequest`。073 落地后「谁往 `OpenSpec.host_tools` 里填」从路由层换成回放，
**装配这一侧一行都不用改**。

**中间态如实记一笔**：062 之后，恢复出来的会话**本身不会带回注入的工具**（声明还没
进 store）；这一次请求要是又带了声明，它会跟 `created` 走同一条 `open` 装进去。
这个中间态**刻意没有测试钉住**——钉住就等于把「宿主必须重连时重报一遍」写成契约，
而 073 要拆掉的正是这个。`existing`（会话还活着）忽略声明这条不变：会话中途换工具表
= 前缀缓存那一刻全断（红线 11），HOST-CAPABILITIES §三 明确不做运行时增删。

> **↑ 这个中间态已于 2026-08-04 由 [073](073-capabilities-into-store.md) 解除。**
> 现在的行为是：声明在建会话时 journaled 地写进 `Slot::HostTools`，恢复出来的会话
> **不带声明也带回它当初那份工具表**（`tests/http_capabilities_survive_restart.rs` 钉住，
> 连 prompt 里的 `tools` 段都逐字节相同）；有历史的会话再带声明一律 **400
> `session_has_history`**。
>
> **062 的接缝经受住了验收**：`assemble::host_tools`、`OpenSpec.host_tools`、
> `ToolTable::with_host_tools` 三处**一行都没改**——073 换掉的只有「谁往
> `OpenSpec.host_tools`/`with_host_tools` 里填」那一行（`actor/body.rs`：恢复时改从
> `session.host_tools()` 取）。这正是本节当初留接口时预期的形状。

### 顺带修的一件事：`packages/protocol/src/generated/` 早就跟 Rust 源漂了

`cargo test -p agent-server --features ts` 一开始红在 `CapabilityTool.ts`（我改了那个
类型的文档注释，注释会进生成的 TS）。重新生成之后发现**另外两个文件本来就是旧的**：
`Command.ts` 缺 `RemoteToolResult` 变体、`SessionEvent.ts` 缺 `AgentTree`/`OrphanFate`
的 import——那是 M8/M9 改完 Rust 之后没重新生成留下的存量漂移，不是本 issue 引入的。
`cargo run -p agent-server --features ts --example gen_protocol_ts` 一并补上（生成物是
派生产物，与 Rust 源一致本来就是那条一致性测试要守的东西）。

### 收工验证（前台跑完，独立 `CARGO_TARGET_DIR` 避开并发会话的 cargo 锁）

`cargo test -p agent-server -p agent-runtime`——**85 个测试二进制，393 passed / 0
failed**，零 warning（数字含并发会话此刻在这两个 crate 里的测试，不全是本 issue 的）：

```
     Running tests/http_capabilities_scoped_to_one_session.rs
running 1 test
test a_declaration_only_reaches_the_session_that_declared_it ... ok
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.10s

     Running tests/http_capabilities_declaration.rs          ← 061 的四条，一条没改
running 4 tests
test the_rejection_message_says_which_item_and_why ... ok
test a_valid_declaration_creates_the_session_and_stays_idempotent ... ok
test omitting_or_emptying_capabilities_keeps_the_old_behavior ... ok
test rejected_declarations_never_create_a_session ... ok
test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.08s

     Running tests/web_tool_never_answered_times_out.rs      ← 060 的那条
running 1 test
test a_web_tool_the_host_never_answers_is_failed_at_its_deadline ... ok
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.35s

  （agent-runtime lib 里新加的六条）
test tool_table::host::tests::injected_tools_are_appended_after_everything_the_sessions_share ... ok
test tool_table::host::tests::the_client_array_order_never_reaches_the_table ... ok
test tool_table::host::tests::a_declared_reversibility_is_taken_as_is_and_location_comes_from_the_prefix ... ok
test tool_table::host::tests::the_injection_map_wins_over_the_name_rules ... ok
test tool_table::host::tests::the_injection_map_wins_over_the_mcp_map ... ok
test tool_table::host::tests::injecting_nothing_changes_nothing ... ok

  （agent-server lib 里新加的四条）
test http::capabilities::assemble::tests::the_three_prompt_facing_fields_are_carried_over_as_is ... ok
test http::capabilities::assemble::tests::a_declared_reversibility_is_used_and_a_missing_one_falls_conservative ... ok
test http::capabilities::assemble::tests::no_declaration_means_nothing_to_inject ... ok
test http::capabilities::assemble::tests::tools_carried_by_a_skill_are_not_injected_yet ... ok
```

`cargo test -p agent-server --features ts`（重新生成之后）：

```
test ts_protocol::consistency::sample_events_cover_every_variant_at_least_once ... ok
test ts_protocol::consistency::fixtures_json_matches_committed_snapshot ... ok
test ts_protocol::consistency::generated_ts_matches_committed_snapshot ... ok
test result: ok. 70 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.24s
```

`cargo clippy -p agent-server -p agent-runtime --all-targets -- -D warnings`
（`-p agent-server --features ts` 同样干净）：

```
    Checking agent-runtime v0.1.0 (/Volumes/work/self/einfach-agent-rust/crates/agent-runtime)
    Checking agent-server v0.1.0 (/Volumes/work/self/einfach-agent-rust/crates/agent-server)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 17.48s
```

`bash scripts/check-invariants.sh --all`：

```
红线检查通过
规则与理由：docs/INVARIANTS.md
```
