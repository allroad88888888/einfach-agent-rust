# 061 `capabilities` 协议类型 + 名字校验（纯数据，零 IO）

**里程碑** M10 · **依赖** — · **模型** sonnet · **独测** —

M10 的第一块，**刻意做成零 IO 的纯数据层**：只定类型、只做校验，不碰会话装配。
这样它能被单独测透，后面 062 的装配才有个确定的地基。接缝见
[HOST-CAPABILITIES.md](../HOST-CAPABILITIES.md) §四。

## 范围

1. **类型**（放 `agent-server/src/http/` 下一个新模块，如 `capabilities.rs`）：

   ```rust
   struct Capabilities { tools: Vec<CapabilityTool>, skills: Vec<CapabilitySkill> }
   struct CapabilityTool { name: String, description: String, schema: serde_json::Value,
                           reversibility: Option<Reversibility> }
   struct CapabilitySkill { id: String, description: String, body: String,
                            tools: Vec<CapabilityTool> }
   ```
   `#[serde(default)]` 用足——缺字段不该是解析失败。`reversibility` 用小写
   （`"pure"`/`"reversible"`/`"irreversible"`），缺省 `None`。

2. **`CreateSessionRequest` 加 `#[serde(default)] capabilities: Option<Capabilities>`**。
   不带时**逐字节向后兼容**（既有测试全绿即证）。

3. **校验函数**（本 issue 的核心，纯函数好测）：
   - **工具名必须 `web:` 或 `desk:` 前缀**（位置从前缀推是既有规则；注入的工具跑在宿主侧）。
     `srv:`、`mcp:`、无前缀 → **拒绝**。
   - 名字字符集与长度：跟 055 的 chatid 同精神——**白名单 + 拒绝，绝不 sanitize**
     （悄悄改写会让两个不同声明撞成一个）。前缀后的部分允许 `[A-Za-z0-9_/-]`。
   - **skill 里自带的 tools 同样过这条校验**（别漏，它们最后也会进工具表）。
   - skill `id` 的字符集（照 `SkillId` 的既有约束）。
   - **重复名字**（tools 内部撞名、skill id 撞名）→ 拒绝，不做「后来居上」。
   - 校验失败返回**结构化错误**，能说清是哪一项、为什么——错误文本要能直接回给调用方。

4. **接进 `POST /sessions`**：解析出来 + 校验，**失败 400 且会话不被创建**。
   校验通过之后**先什么都不做**（装配是 062 的事）——本 issue 到此为止。

## 验收（可判定）

- 合法声明 → 201/200 正常建会话（此刻工具表还没变，那是 062）。
- `srv:x/y`、`mcp:x/y`、`nopfx`、空名 → **400**，且 **registry 里没有这个会话**（真断言，
  不能只断言状态码）。
- skill 里自带一个 `srv:` 前缀的工具 → 同样 400（这条最容易漏）。
- tools 内部撞名 / skill id 撞名 → 400。
- `reversibility` 缺省 → 解析成 `None`（062 会把它落成保守 `Irreversible`）。
- 不带 `capabilities` 的老调用方：**既有 `POST /sessions` 测试一条不改地全绿**。

## 注意

- **零 IO、零装配**：本 issue 不碰 `OpenSpec`/`ToolTable`/`SessionTemplate`。范围守住了，
  062 才好做。
- **不要碰** `crates/agent-tools/`（并发会话 WIP）；`location_of` 的名字规则别动
  （050 刚在 `agent-runtime::tool_name` 落地）。
- 红线 9：≤300 行。`sessions.rs` 现在 167 行，加解析+校验可能顶破 → **校验单独成模块**
  （本来就该，纯函数好测）。
- 收工验证前台跑完（WORKFLOW §四 -1）；主 target 被占就用独立 `CARGO_TARGET_DIR`。

## 实做记录（完成 · 2026-08-04）

### 建了什么（行数为改后，红线 9 全部 ≤300）

| 文件 | 行 | 干什么 |
|---|---|---|
| `crates/agent-server/src/http/capabilities/mod.rs` | 221 | **协议形状**：四个类型 + serde 默认值 + 小写可逆性到 `agent_core::Reversibility` 的 `From` |
| `crates/agent-server/src/http/capabilities/validate.rs` | 293 | **名字校验**：规则、结构化拒绝理由、纯函数 `validate` |
| `crates/agent-server/src/http/routes/sessions.rs` | 187 | `CreateSessionRequest` 加 `capabilities` 字段 + 一处调用（167 → 187） |
| `crates/agent-server/src/http/mod.rs` | 132 | 挂模块、模块地图一行、`ts` 门后面的再导出 |
| `crates/agent-server/src/ts_protocol/export.rs` | 76 | 多导一个 `Capabilities`（连同它递归的三个类型） |
| `crates/agent-server/tests/http_capabilities_declaration.rs` | 144 | 四条端到端：合法建会话 / 逐条拒绝 / 错误文案 / 向后兼容 |

模块拆成两个文件是**职责拆**不是凑行数：`mod.rs` 只回答「宿主能说什么」（形状，改它就是改
协议），`validate.rs` 只回答「什么样的名字收不下」（规则，纯函数、不认识 HTTP 也不认识
registry）。

### 类型形状

```jsonc
{ "capabilities": {
    "tools":  [ { "name": "web:crm/lookup", "description": "…",
                  "schema": { … }, "reversibility": "pure" } ],
    "skills": [ { "id": "crm-flow", "description": "…", "body": "…",
                  "tools": [ … 同上 … ] } ] } }
```

- **`#[serde(default)]` 用足**：`{}`、`{"tools":[]}`、少写 `description`/`schema`/`tools`
  全部解析成功。`schema` 缺省 `{"type":"object"}`——照 `agent_runtime::skill::load` 装
  SKILL.md 时的既有兜底，不是新发明的默认值。
- **连 `name`/`id` 也有默认值（空串）**。名字缺了当然要拒，但那一拒要由 `validate` 用
  「哪一项、为什么」的话来拒；让 serde 去拒只会得到 `ApiJson` 那句通用的「请求体字段形状
  跟期望的不符」，调用方看不出是哪个工具写坏了。
- **`reversibility` 是小写字符串**，所以有一个自己的 `CapabilityReversibility`，不复用
  `agent_core::Reversibility`——后者 serde 出来是 PascalCase，且已经落进会话 jsonl 和
  `ToolCallRequest` 的 TS 类型，改它等于改存量数据格式。两种拼法之间只隔一个 `From`
  （062 装配时用它）。缺省解析成 `None`：**协议层不替宿主把「没说」解释成 `Irreversible`**，
  那是 062 的事。
- 认不得的字段忽略、不报错——宿主比 server 先升级是常态。

### 校验规则表

| 项 | 规则 | 拒绝理由类型 |
|---|---|---|
| 工具名前缀 | **必须** `web:` / `desk:`；`srv:`、`mcp:`、无前缀、空名一律拒 | `ToolPrefix` |
| 工具名前缀之后 | 非空、只许 `[A-Za-z0-9_/-]`、全名 ≤128 字节 | `ToolNameShape` |
| **skill 自带的工具** | **同一条规则**（`check_tool` 是同一个函数，只换 `Origin`） | 同上两条 |
| skill `id` | 非空、只许 `[A-Za-z0-9_-]`、≤128 字节（同 055 的 chatid 一档） | `SkillIdShape` |
| 工具名重复 | 顶层之间、skill 之间、skill 与顶层之间——**全局唯一** | `DuplicateTool` |
| skill id 重复 | 拒，不做「后来居上」 | `DuplicateSkill` |

工具名的唯一性刻意是**全局**的（顶层的和每个 skill 自带的进同一个 `BTreeSet`）：它们最后
进的是同一张工具表，重名在哪儿发生都是同一个问题。skill id 不许 `/` 和 `:`，是因为它每行
一个进常驻索引（`<id>: <描述>`），字符集收到 chatid 那一档，索引那一行就不可能被 id 里的
分隔符或换行撑破。

### 为什么拒绝而不是 sanitize

同 055 的 chatid：悄悄把 `web:a b` 洗成 `web:a_b`，两个本来不同的声明就撞成同一个工具
名——后一个静默盖掉前一个，模型调到哪一个取决于数组顺序。**静默串工具比拒绝更坏。**
同一条理由也是「重名不做后来居上」的理由：宿主自己都没想清楚要哪个，server 替它选一个
只是把问题推到运行时（那时症状是「模型调了个行为不对的工具」，离现场十万八千里）。

前缀白名单只收 `web:`/`desk:` 的理由是位置：`location_of` 从前缀推执行位置，注入进来的
工具本来就跑在宿主侧，用这两个既有前缀就直接接上「推 `ToolExecuting` → 宿主
`POST /tool_result`」那条已经通了的通道，**零新代码**；`srv:`/`mcp:` 会被判成服务端执行，
dispatch 去本进程里找一个根本不存在的实现。`location_of` 一个字节没动（050 刚落地）。

错误文案回显出错的那个名字（说得清是哪一项），但**按 64 字符截断**——`crate::http::json`
里那条「错误响应不该变成把请求体原样弹回去的信道」同样适用于这里。

### 「会话没被创建」是怎么断言的：反向验证过它真的会红

`rejected_declarations_never_create_a_session` 每一条坏声明都断言三件事：400、错误体是
统一的 `"bad_request"` 形状、**`sessions.ids()` 为空且 `GET /sessions/<chatid>` 是 404**
（不是「存在但死了」）。只断状态码不算数——把校验从 `open` 之前挪到 `open` 之后（400 照
返回，但会话已经建出来了）跑一次，测试在**第二条断言**上红：

```
---- rejected_declarations_never_create_a_session stdout ----
panicked at crates/agent-server/tests/http_capabilities_declaration.rs:90:9:
srv: 前缀：不合规的声明不得登记 session：[SessionId("capabilities-chat")]
```

也就是说这条断言真的在守「拒绝发生在任何 `open_spec`/`open` 之前」这件事，而不是跟着
状态码一起白拿。校验点落在 `create` 里 chatid 白名单**紧挨着的下一行**——同一段、同一
套「白名单 + 拒绝，绝不 sanitize」，坏声明在文件系统和 registry 上都留不下痕迹。

### ts-rs：`Capabilities` 导出了，065 不用手写镜像

`export_protocol_types` 多点名一个 `Capabilities`，递归带出 `CapabilityTool`/
`CapabilitySkill`/`CapabilityReversibility`（`schema` 复用既有的 `serde_json/JsonValue`）。
这是目前唯一从 `packages/protocol` 出去的**请求体**类型，`src/index.ts` 收拢了这四个名字。

TS 形状按 wire 的真话生成，不是 Rust 结构的直译：`#[serde(default)]` 的数组用
`#[ts(optional, as = "Option<Vec<…>>")]` 落成 `tools?:`/`skills?:`，`reversibility` 落成
`reversibility?: "pure" | "reversible" | "irreversible"`——否则前端为了过类型检查得写一串
`skills: []`，那正是「协议类型从 Rust 生成」要避免的偏差。（`packages/web/src/capabilities/
wire.ts` 是 065 在 061 之前手写的临时定义，形状与这份生成物一致，可以删掉改成从
`@agent/protocol` 导入——那是 065/067 那边的收尾。）

### 装配一行没写

`OpenSpec`/`ToolTable`/`SessionTemplate`/`SkillRegistry` 一个字节没碰，`agent-tools`/
`agent-runtime` 一个字节没碰。校验通过之后 `capabilities` 就被丢掉——`CapabilityTool` /
`CapabilitySkill` 上那句 `#[allow(dead_code)]` 就是这件事的记账（除 `name`/`id` 外的字段
此刻没有读者），**062 装配上去时应当连同这条 allow 一起删掉**。

### 收工验证（前台跑完，独立 `CARGO_TARGET_DIR` 避开并发会话的 cargo 锁）

`cargo test -p agent-server`——36 个测试二进制，**161 passed / 0 failed**，零 warning
（数字含并发会话此刻在这个 crate 里加的测试，不全是本 issue 的）。新加的四条 + 055
原封不动的四条：

```
     Running tests/http_capabilities_declaration.rs
running 4 tests
test the_rejection_message_says_which_item_and_why ... ok
test a_valid_declaration_creates_the_session_and_stays_idempotent ... ok
test omitting_or_emptying_capabilities_keeps_the_old_behavior ... ok
test rejected_declarations_never_create_a_session ... ok

test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.04s

     Running tests/http_chatid_sessions.rs
running 4 tests
test omitting_chatid_keeps_the_legacy_generated_id_and_created_status ... ok
test invalid_chatids_are_rejected_before_creating_any_session_file ... ok
test repeated_chatid_reattaches_to_the_live_session_without_clearing_history ... ok
test closed_chatid_recovers_history_from_its_default_session_file ... ok

test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.06s
```

`cargo test -p agent-server --features ts`（证明 TS 已重新生成、跟 Rust 源一致）：

```
test ts_protocol::consistency::sample_events_cover_every_variant_at_least_once ... ok
test ts_protocol::consistency::fixtures_json_matches_committed_snapshot ... ok
test ts_protocol::consistency::generated_ts_matches_committed_snapshot ... ok
test result: ok. 65 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.17s
```

`cargo clippy -p agent-server --all-targets -- -D warnings`（`--features ts` 同样干净）：

```
    Checking agent-server v0.1.0 (/Volumes/work/self/einfach-agent-rust/crates/agent-server)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 2.36s
```

`bash scripts/check-invariants.sh --all`：

```
红线检查通过
规则与理由：docs/INVARIANTS.md
```
