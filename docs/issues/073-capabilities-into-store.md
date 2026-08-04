# 073 注入的声明进 store：恢复时原模原样复刻，而不是重新注入

**里程碑** M10 · **依赖** 062 · **模型** opus · **独测** ✅（碰红线 3、恢复一致性）

用户拍板（2026-08-04）：

> 历史对话记录，不用对工具再注入一次。**历史对话就该跟历史一致，原模原样 100% 复刻。**

## 为什么必须这样（三条，缺一条都不够）

1. **历史对话是在那一份工具表下产生的。** 模型当初的消息里写着「我调用了
   `web:crm/lookup`」，如果恢复时装的是前端**今天**的清单（可能已经删了这个工具、加了别的），
   历史就自相矛盾——模型看到自己调过一个"不存在"的工具。
2. **红线 11。** 工具表在 prompt **最前面**。恢复时换一份 = 第一轮就前缀全断，而恢复出来的
   会话**本该能接着用缓存**（M2 起「恢复 = redo」就是这个承诺）。
3. **跟本仓核心哲学一致。** undo / redo / 崩溃恢复 / 审计是同一套机制的**四个投影**
   （CLAUDE.md 第一句）。**恢复是忠实重放，不是「用今天的配置重建」。** 把 per-session 的
   注入当部署配置，等于在这套投影里开了一个洞。

## 现状（062 之后的中间态 · **已由本 issue 解除**）

062 让声明从 HTTP 请求装进 `ToolTable`，但**声明本身没有落盘**——`ToolTable` 活在
`RunnerCtx`（运行时），不进 store。于是恢复出来的会话**没有注入的工具**，而且不报错。
本 issue 填掉这个洞（实做记录在文末；062 的实做记录里那段中间态已标注解除）。

## 范围

1. **声明进 store**：给会话加一个 primitive slot（如 `Slot::Capabilities`），建会话时
   **journaled 地写一次**。形状是 061 定的 `Capabilities`（`name`/`description`/`schema`/
   `reversibility`，全部可序列化——**红线 3 满足**）。
2. **恢复时自动回来**：走既有的「恢复 = 从快照把 `Entry` 按 `next` 往前推」那条路
   （STATE-MODEL §恢复），**不需要新机制**。actor 起来后装配 `ToolTable` 时读这个 slot。
3. **前端不必再声明**：恢复出来的会话，客户端**不带 `capabilities` 也该有那些工具**。
   带了呢？——见下面「直接拒绝」那一节（已落地）。
4. **与 skill 的既有模式对齐**：skill 是「激活状态在 store（`SkillsActive`，journaled）+
   内容在运行时 registry」。注入的能力是「**声明**在 store + **执行**在宿主侧」——同构，
   不是新发明。

## 验收（可判定）

- **恢复 100% 复刻（本 issue 的全部意义）**：建会话 + 注入 `web:x/y` → 对话一轮 →
  关掉会话 → 同 chatid 重开、**不带任何 `capabilities`** → 工具表里**仍然有 `web:x/y`**、
  `declares()` 为真、`snapshot()` 的可逆性跟当初一致。
- **prompt 前缀一致**：恢复后第一轮的工具表**字节**与关闭前最后一轮相同（红线 11 的真意
  在这里——不只是"有这个工具"，是"逐字节一样"）。
- **undo 也一致**：注入发生在会话建立那一步，`undo` 到它之前 → 工具表回到没有注入的状态
  （这条白拿，因为走的是同一条 journaled 路——但要有断言证明它真的白拿了）。
- 既有测试全绿：061 的四条、062 的装配断言、055 的 chatid 四条。

## 恢复时客户端又带了 `capabilities` → **直接拒绝**（用户拍板 2026-08-04）

不忽略、不比对、不合并——**400，明确告诉它「这个会话已有历史，能力从历史来，别再声明」**。

为什么是这条而不是「忽略」或「不一致才报错」：

- **忽略**会制造本仓最讨厌的那种 bug：前端以为登记上了，其实没有，没有任何报错，
  症状是"模型死活不用某个工具"，离现场十万八千里。
- **不一致才报错**要先定义"一致"（逐字节？名字集合？描述算不算？），**每一种定义都会有
  人踩到边界**；而且它默认了"一致时可以重复声明"，等于给"重新注入"留了个后门。
- **直接拒绝**没有歧义：能力属于历史，历史不接受改写。跟 055 的 chatid**拒绝而不 sanitize**、
  061 的重名**一律拒绝不做后来居上**是同一条取向。

### 这条给客户端带来的契约（**必须写进前端文档**）

前端不知道一个 chatid 是新的还是有历史的，而"带声明"和"不带声明"的正确选择取决于它。
两条出路，**在实现时选一条并写进 065 的 README**：

1. **先查再建**：`GET /sessions/{id}` → 404 就带声明建、200 就不带。多一次往返，但语义直白。
2. **乐观带 + 按错误码降级**：带声明 POST，收到这个特定的 400 就不带声明重试一次。
   少一次往返，但要求这个拒绝有**可判别的错误码**（不能只是通用 `bad_request`）——
   **本 issue 要提供那个错误码**。

**倾向 1**（少一条"靠错误码驱动控制流"的路径），但 2 也可接受；无论选哪条，
**拒绝的错误体必须能让客户端区分"我名字写错了"和"这会话已有历史"**。

## 注意

- **红线 3**（primitive 的值必须全部可序列化）：`Capabilities` 满足，但要确认 `schema`
  那个 `serde_json::Value` 落盘/回放是逐字节稳定的（`serde_json::Map` 是 `BTreeMap`，
  根 `Cargo.toml` 显式不开 `preserve_order`——**但要写一条断言**，别假设）。
- **红线 4**（快照与日志用 `AtomKey` 不用 `AtomId`）：新 slot 照既有形状。
- **红线 11**：装配顺序、内部排序的规则由 062/063 定死，本 issue 不改，只保证**恢复后
  跟当初一样**。
- **不要顺手做「运行时增删」**（接缝 §三 明确不做）。本 issue 只让恢复忠实，不开中途改的口子。
- 碰 `agent-core` 的 command 层，**要独立测试 agent**（红线 3 + 恢复一致性都属于「错了不
  报错、只在恢复时以静默错值浮出来」那一类）。
- 收工验证前台跑完（WORKFLOW §四 -1）。

## 实做记录（完成 · 2026-08-04）

### slot 长什么样

```rust
Slot::HostTools          // Slot::ALL 的第 12 个，追加在末尾（旧快照缺键 → 默认值，白拿）
默认值 = AgentValue::Json([])                       // 同 SkillsActive：永远持一个数组
可见性 = Visibility::Upward                          // 同 SkillsActive：会话级的上下文资产
落盘形状 = [{ "name","description","schema","reversibility" }, …]   // 按 name 排序
```

值的编解码收在 `value::host_tools`（跟 `value::str_set` 并列，同一个存在理由：**排序这一步
不能在两个地方各写一遍**）。**一项就是一个自洽的对象**——不做「一个 `ToolSpec` 数组 + 一个
旁挂的可逆性数组」，那会让两个数组的下标对齐变成一条没人检查的纪律，历史数据一旦错位就是
静默错值。

跟 skill 的模式**同构但存的东西不同**，差别的根源是「store 外面有没有第二份」：skill 的
正文在 `SkillRegistry`（本机装载的资产，两次运行之间可能改），所以 store 只存激活的 id；
宿主注入的声明**只在那一次 HTTP 请求里存在过**，不存下来就没有第二处可取——而向宿主重新
要一遍正是本 issue 要拆掉的东西。

| 文件 | 行 | 干什么 |
|---|---|---|
| `crates/agent-core/src/value/host_tools.rs` | 165 | **新**：声明 ↔ `AgentValue::Json` 的唯一一处编解码（排序、跳过坏项）+ 4 条单测 |
| `crates/agent-core/src/command/host_tools.rs` | 125 | **新**：`declare_host_tools` / `host_tools`（journaled 读写）+ 2 条单测 |
| `crates/agent-core/src/graph/slot.rs` | 214 | 加 `Slot::HostTools` + 默认值 + `ALL`（11 → 12） |
| `crates/agent-core/src/graph/visibility.rs` | 161 | 站队 `Upward`（红线 10 的穷举 match 逼着回答） |
| `crates/agent-core/src/command/meta.rs` | 115 | `KNOWN_LABELS` 加 `declare_host_tools` |
| `crates/agent-server/src/actor/body.rs` | 276 | **本 issue 的全部改动面**：声明从哪来 + 新建时写一次（见下） |
| `crates/agent-server/src/http/routes/sessions.rs` | 259 | 有历史 + 带声明 → 拒；`GET` 认识 `dormant` |
| `crates/agent-server/src/http/error.rs` | 62 | `ApiError::session_has_history`（可判别错误码） |
| `crates/agent-core/tests/host_tools_indep_restore.rs` | 173 | **新**：恢复路径 5 条（含逐字节 serde 往返、游标在声明之前） |
| `crates/agent-server/tests/http_capabilities_survive_restart.rs` | 261 | **新**：端到端 4 条（复刻 / 逐字节 / 拒绝 / undo） |

### 装配那一侧：**一行都没改**（本 issue 的关键验收）

062 留的三处接缝原封不动：`http/capabilities/assemble.rs`（声明 → `Vec<(ToolSpec,
Reversibility)>`）、`registry/spec.rs` 的 `OpenSpec.host_tools`、
`agent-runtime/src/tool_table_host.rs` 的 `ToolTable::with_host_tools`——**三个文件本次
一个字节没动**（`http/config.rs` 的 `open_spec` 签名也没动）。

073 换掉的就是 `actor/body.rs` 里的一行「谁往里填」：

```rust
// 062：                 spec.tools.build().with_host_tools(spec.host_tools)
// 073：
let host_tools = if restored { session.host_tools() } else { spec.host_tools.clone() };
//                            ↑ 从回放出来的会话状态里取     ↑ 新建才看这次请求
                       spec.tools.build().with_host_tools(host_tools)
```

接缝留对了：装配那一侧的入参本来就是「一份声明」而不是「一次 HTTP 请求」，所以换来源
不需要它知道。

### 写入点：`seed_after_recover` **之后**，而且声明**自成一轮**

```rust
agent_runtime::persist::seed_after_recover(&mut ctx, &session);
if !restored && !spec.host_tools.is_empty() {
    session.declare_host_tools(spec.host_tools);   // 一条 journaled entry（turn 1）
    session.begin_turn();                          // 见下：不落 entry，只推 turn 边界
    agent_runtime::persist::sync(&mut ctx, &mut session);
}
```

两条顺序都是**踩出来的**，不是想出来的：

1. **必须排在 `seed_after_recover` 之后。** 那一步的语义是「`session` 里现有的条目本来
   就在盘上」，声明这条是本轮新写的；排在它前面就会被当成「已同步」，从此**永远不落盘**
   ——会话文件里没有它，下次恢复整份声明静默消失。这正是 `persist::sync` 文档里记的那个
   「真 bug」的同款形状。突变②验证过：这么改，核心那条测试当场红。
2. **声明自成一轮。** `TurnStatus::Idle` **不是终态**，所以第一轮对话不会自己
   `begin_turn`——不推这一下，声明和第一轮对话共用 turn 1，用户 `/undo` 撤掉第一句话
   会**连宿主的声明一起撤掉**，而且要等到下次重开会话、工具表少几个才看得出来。
   这条是写 undo 测试时**当场炸出来的**（观察 B 红），不是预先想到的。
   对刚建好的会话，`begin_turn` 自己一个 `Change` 都不产生（状态已经是 `Idle`、工具槽
   已经是空、预算已经是 0），`History::append` 拒绝空步 → **不落 entry**，作用只有一个：
   把 turn 边界推过去。

**不带 `capabilities` 的会话整段不做**：一条 entry 不多、turn 号也不动，会话文件跟 073
之前逐字节相同。

### 拒绝：`session_has_history`，两道闸

| 层 | 判据 | 结果 |
|---|---|---|
| HTTP 路由 | `<sessions-dir>/<chatid>.jsonl` 在 **且** 请求带 `capabilities` | **400 `session_has_history`** |
| actor（第二道） | `restored` **且** `spec.host_tools` 非空 | `open` 失败（409），不静默忽略 |

- **错误码可判别**：调用方要能把「我工具名写错了」（`bad_request`，改名重发）和「这会话
  已有历史」（`session_has_history`，去掉声明重发）分开——两者都是 400，正确的应对相反。
- **有历史这条排在名字校验前面**：这一次的声明无论写得多正确都不会被采纳，先报「你根本
  不该带它」比先报「你第三个工具名不合规」有用（后者会让人以为改个名字就能过）。
- **第二道闸不是多余的**：它让「恢复不接受新声明」成为 actor 的性质而不是「路由记得检查」。
  突变⑦（删掉路由那道）验证过：请求仍然硬失败（409 + 说人话的理由），只是错误码没那么好用
  ——**任何路径下都不会静默忽略**。

### 客户端契约：先查再建（**为此 `GET /sessions/{id}` 多认一态**）

`GET /sessions/{id}` → **404 就带声明建、200 就不带**。落地时发现一个坑：这个端点原来只
问 registry，于是**「关掉了但磁盘上有」——恰恰就是恢复那种情况**——会被答成 404，客户端
据此判定「新会话」带上声明，然后被拒绝顶回来，**契约当场作废**。所以补了第三态
`dormant`（registry 没有、会话文件在，也就是下一次 POST 会走恢复）。404 的含义因此收敛成
一句可以直接当判据用的话：**这个 chatid 没有任何历史**。

契约写进了 [INTEGRATION.md](../INTEGRATION.md) §三「安全点三」（**网关作者读那一份**——
真正复用 chatid 的宿主是 Java 网关）和 [065](065-frontend-inject.md) 的追记。
**`packages/web/` 一个字没改**：它每次开页都建全新会话（`createSession` 不收 chatid、
id 不落 localStorage），永远走「新建」那一支，天然合规。

### 测试：先红后绿 + 每条断言都做过突变

核心那条**改之前的红色输出**（062 的中间态：恢复出来没有注入的工具）：

```
test a_recovered_session_brings_its_declared_tools_back_without_being_told_again ... FAILED

assertion `left == right` failed: 恢复出来的会话该带回它自己当初的那份工具表，宿主不必也不该再声明一遍
  left: ["srv:fs/read", "srv:fs/list"]
 right: ["srv:fs/read", "srv:fs/list", "desk:clipboard/write", "web:crm/lookup"]
```

九个突变，每个都**真的跑红过、也真的改回来了**（收工 `grep -rn "MUTATION" crates/` 无残留）：

| # | 改坏哪一行 | 谁红 |
|---|---|---|
| ① | `body.rs` 不写 store | 核心那条：`left: ["srv:fs/read","srv:fs/list"]` |
| ② | 声明挪到 `seed_after_recover` **之前** | 同上（声明没落盘） |
| ③ | 恢复时也用请求里的声明（= 用户否决的做法） | 同上 |
| ④ | 去掉 `begin_turn`（声明与第一轮对话共用 turn） | undo 那条的**观察 B**（正对照） |
| ⑤ | codec 去掉按名字排序 | `the_bytes_do_not_depend_on_input_order_anywhere` + 另两条 |
| ⑥ | codec 落盘时丢掉 `description` | 独测两条 **+ 端到端「逐字节一致」那条**（恢复后的 prompt 里 `description` 全空） |
| ⑦ | 路由不拒绝 | 拒绝那条：`left: 409 right: 400`（第二道闸接住了，但码不对） |
| ⑧ | `GET` 去掉 `dormant` | 契约那条：`left: 404 right: 200` |
| ⑨ | 拒绝判据换成「registry 里活着」（漏掉恢复这一支） | 拒绝那条：`left: 409 right: 400` |

**四条验收各自的落点**：

- **恢复 100% 复刻**：`a_recovered_session_brings_its_declared_tools_back_without_being_told_again`
  ——第二次 `POST /sessions` 请求体里**一个 `capabilities` 字都没有**，断言落在假上游
  收到的 `tools` 数组上（工具表最终的用处就是变成 prompt 字节，断在这里才算真的证到）。
- **prompt 前缀逐字节一致**：同一条测试的第二个断言，比的是两次 `tools` 段
  `serde_json::to_string` 的**字符串相等**，不是「有没有这个工具」。突变⑥专门验证过它
  抓得住「字段掉了但工具还在」。
- **undo 也一致**：`undoing_past_the_declaration_takes_the_injected_tools_out_of_the_table`
  ——三次观察，**中间那次是正对照**：A（有注入）→ undo 掉对话那一轮、重开 → B **仍有**
  注入 → 再 undo 两轮、重开 → C **没有**注入。只断言 C 是自欺欺人：一个「从来就没恢复过
  任何工具」的实现同样会绿。core 层另有一条同款（`a_log_whose_cursor_sits_before_the_
  declaration_restores_without_it`，也带正对照）。
  「重开」这一步省不掉：工具表在 actor 起来时装配一次，之后这个运行实例内不再变
  （§三 不做运行时增删），undo 改的是会话状态，对表的作用要等下一次装配才看得见。
- **`schema` 逐字节稳定**：`the_declaration_survives_a_serde_roundtrip_byte_for_byte`
  （整份快照往返 + 两次序列化字符串相等）与 codec 那条（两种相反的键插入顺序 + 相反的
  数组顺序 → 同一份字节）。它会红的那一天是有人给某个依赖打开了 `preserve_order`。

### 既有测试里被这个 slot 顶红、如实改掉的槽位计数

`Slot::ALL` 11 → 12，于是「每个 agent 几个槽位」的既有断言集体要改（`build_agent` 对
root 和子 agent 是同一个函数，所以子 agent 也各有一个空的 `HostTools`）：
`session_state.rs`（11 → 12）、`session_indep_snapshot_shape.rs`（`EXPECTED_SLOT_COUNT`
11 → 12）、`subagent_indep_snapshot.rs`（33 → 36）、`subagent_indep_despawn.rs` /
`subagent_indep_tombstone.rs`（逐出 10 → 11）、`subagent_indep_undo_spawn.rs`（11 → 12）、
`subagent_indep_visibility.rs` 与 `graph/visibility.rs` 的 `Upward` 名单（加 `HostTools`）、
`session_indep_accounting.rs` 的穷举 match。**这些红是这些测试的价值本身**——它们就是
「顺手加一个槽位而没想清楚它进不进快照」的看门狗，改数字前逐条问过一遍。

### 收工验证（前台跑完，独立 `CARGO_TARGET_DIR`）

```
cargo test -p agent-core -p agent-server -p agent-runtime
  → 140 个测试二进制，756 passed / 0 failed（含并发会话此刻在这几个 crate 里的测试）

cargo test -p agent-server --features ts
  → 全绿（本 issue **没有改协议类型**：新错误码是响应体里的一个字符串，
    `SessionStatusResponse` 不在 ts-rs 导出面上，`generated/` 一个文件都不用重生成）

cargo clippy --all-targets -- -D warnings   → Finished，零 warning
bash scripts/check-invariants.sh --all      → 红线检查通过
```

### 两个如实记下的边角

1. **「有历史」的判据是「磁盘上有会话文件」**，不是「store 里真的有声明」。于是一个
   建了、没说话、就关掉的 chatid（文件在、里面没有声明）之后也不能再带声明了。这是刻意
   选的保守面：判据跟 `outcome: "recovered"` **用的是同一个**，「会不会走恢复」和「能不能
   带声明」因此永远一致，不会出现「它说 recovered、却又接受了新声明」这种自相矛盾。
2. **纯内存会话（没有 `default_sessions_dir`）沿用 062 的行为**：`existing` 那一支忽略
   这次的声明。它们没有历史可复刻，061 的幂等测试也依赖这条；真正需要拒绝的是「有历史」，
   而内存会话一进程一世，没有恢复这回事。
