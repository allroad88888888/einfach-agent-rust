# 076 建会话时挑选内置能力：哪些默认工具这个会话能看见

**里程碑** M10 追加（用户 2026-08-04 提出） · **依赖** 073（声明进 store）+ 075（`push_spec`） · **模型** opus · **独测** ✅（红线 11 + 恢复一致性）

## 用户原话与拍板

> 初始化一个 chat 时候，可以加一些参数控制哪些默认的 skills 启用，哪些不启用。

追问后确认了三件事（**这三条是本 issue 的边界，别自己扩**）：

1. **「默认那批」= 内核自带的那批工具**，不是磁盘上的 skill 库。也就是 server 起来时那句
   `tools=builtin+shell+spawn` 打出来的东西——`ToolTableSpec` 的五档装配
   （`builtin`/`with_shell`/`standard_local`/`standard` + `with_spawn`/`with_status`/
   `with_collect`/`with_skills`）。**064 刚拍的「server 不从磁盘装载 skill」不受影响，别去动它。**
2. **「启用/不启用」= 控制模型看不看得见**：不启用的那些**连名字和描述都不进 prompt**，
   模型压根不知道有它。不是「看得见但不给调」，也不是「预先激活正文」。
3. **子 agent 不单独配**：整棵 agent 树共用会话级的这一套。spawn 时不加参数。

## 为什么这是个真需求

今天这个选择是**部署级**的：一个 server 进程起来打成什么档，它上面所有会话就都是那个档。
但同一个部署上的会话用途可以完全不同——一个纯问答的客服会话不需要 `srv:agent/spawn`，
一个只读分析会话不该看见 `srv:shell/exec`。今天要么整个部署关掉（别的会话跟着遭殃），
要么起两个进程。

## 两条不商量的约束（不是新讨论，是既有判据的直接推论）

### 一、**只能减，不能加**

请求里给的是「**关掉哪些**」，不是「开哪些」。部署方装配出来的那张表是**天花板**，
会话只能在它下面挑。

理由不需要新的安全讨论就成立：反过来（客户端说「给我开 `srv:shell/exec`」）意味着
**前端一句 JSON 就能突破部署方的决定**，而 `capabilities` 这条路上的客户端是浏览器
（可被终端用户改 JS、可被 XSS）。接缝 §九 那段「安全暂缓」讨论的是**注入新能力**的策略，
不是这条——这条方向上根本没有可讨论的余地。

### 二、**开关进 store，跟 073 同一条路**

选择是**会话状态**，建会话时 journaled 地写一次，恢复时从日志回放。**不是部署配置。**

理由就是 073 那三条原样成立（`docs/issues/073` §为什么必须这样）：历史对话是在**那一张表**下
产生的；工具表在 prompt 最前面（红线 11）；恢复是忠实重放，不是用今天的配置重建。
落地照 `Slot::HostTools`/`Slot::HostSkills` 的既有形状，**别新发明**。

推论：**已有历史的会话再带这个字段 → 400 `session_has_history`**，跟 073 完全同一条闸，
不要单开一条。

## 范围

1. **协议**：`POST /sessions` 的 `capabilities` 加一个字段（建议 `disable_builtin: Vec<String>`，
   名字自己定，但要一眼看出是**减法**）。缺省空数组 = 今天的行为，**逐字节不变**。
2. **校验（在最早能报给作者的点上失败，069 的判据）**：列表里的名字**必须在这个部署
   实际装配出来的表里**。不认识的名字 → **400**，报文点名是哪个。
   - 为什么不静默忽略：拼错一个名字 → 客户端以为关掉了、其实没关 → 模型照样调得到
     `srv:shell/exec`，**没有任何报错**。这正是本仓最贵的那类。
   - 那一刻客户端还在线，能改，所以该在那里失败（对比 064 的 `skill_injection`
     过滤——每轮都跑、作者早不在场，所以那里绝不能报错）。
3. **装配**：在五档装好之后、`with_mcp`/`with_skills`/`with_host_tools` **之前**过滤掉。
   注入的东西不受这个开关影响（那是宿主自己声明的，不是「默认那批」）。
4. **进 store + 恢复回放**：照 073。`actor/capabilities.rs`（064 拆出来的）是这件事的落点。

## 验收（可判定）

- **关掉就真看不见**：关掉 `srv:agent/spawn` → 那个会话的 `declares("srv:agent/spawn")` 为假、
  它**不在进 prompt 的那份字节里**；同一个 server 上另起一个不关的会话 → 照旧有。
- **只能减不能加**：请求里写一个部署没装配的名字 → **400**，报文点名。
  （**这条要有测试**，别只在文档里说。）
- **恢复原模原样**：带开关建会话 → 对话一轮 → 关掉 → 同 chatid 重开、**不带任何 capabilities**
  → 被关掉的那些**仍然是关掉的**，且**工具表逐字节与当初相同**（红线 11 的真意）。
- **已有历史再带 → 400 `session_has_history`**（跟 073 同一条闸，不是新错误码）。
- **红线 11 确定性**：同一份关闭列表**打乱顺序** → 工具表字节相同（列表顺序不可以泄漏进
  prompt）。**删掉排序/去重 → 红**（突变验证，贴输出）。照 063 刚落的两个测试文件的范式。
- 既有测试全绿；**不带这个字段时工具表与今天逐字节相同**（这条要有断言）。

## 诚实的代价：前缀家族会变多

内置那一段今天**所有会话完全相同**，于是同一部署上的会话在上游那边共享同一个前缀缓存。
每一种不同的关闭组合就是**一个不同的前缀家族**——组合多了，跨会话的缓存复用会碎。

这不是不做的理由（会话内的前缀稳定性一点没变，红线 11 说的是那个），但**要写进文档**，
并建议宿主**收敛到少数几个固定组合**，而不是每个 chat 随手勾一份不一样的。

## 注意

- **不要顺手做「运行时增删」**：跟 062/073 一样，只在建会话那一次定，中途不改口。
- **不要碰** 064 的磁盘装载决定、`skill_injection` 的过滤、`agent-tools/`、`agent-mcp/`。
- 子 agent **不加参数**（用户明确说暂时不要）；但要在实做记录里写一句：整棵树共用会话级
  这一套，`spawn` 一行没改。
- 红线 9：≤300 行。**`tool_table.rs` 075 之后正好 300 行，零余量**——你要加过滤逻辑，
  **拆分就是本次改动的一部分**，按职责拆（照 062 的 `tool_table_host.rs`、064 的
  `tool_table_skill.rs` 的先例：拆的是「一件事」，不是切后半截）。
- 收工验证前台跑完（WORKFLOW §四 -1），含 `--features ts`（协议面变了，生成物要一起）。

## 实做记录（完成 · 2026-08-04）

### 字段与槽位

```jsonc
POST /sessions  { "capabilities": { "disable_builtin": ["srv:agent/spawn", "srv:shell/exec"] } }
```

```rust
Slot::DisabledBuiltins   // Slot::ALL 的第 14 个，追加在末尾（旧快照缺键 → 默认值，白拿）
默认值 = AgentValue::Json([])                 // 空 = 一个都没关 = 今天的行为
可见性 = Visibility::Upward                    // 整棵树共用会话级这一份（见下「子 agent」）
落盘形状 = ["srv:agent/spawn", "srv:shell/exec"]   // 排序去重
```

**值的编解码没有新文件**：它就是「一组字符串，排序去重后落成 JSON 数组」，跟
`Slot::ToolsAllowed`（028）、`Slot::SkillsActive`（039）**同一个形状**，所以直接用
`value::str_set` 那一处既有编解码——那个模块的存在理由原话就是「排序去重这一步不能在两个
地方各写一遍」，这次让它从两个用户变成三个，而不是照 073/064 的样子再抄一份 codec。
073 的 `host_tools`/064 的 `host_skills` 各有自己的 `value/*.rs` 是因为它们的项是**对象**
（name/description/schema/…），这一个不是。

### 校验在哪一跳：HTTP 路由，**在 `open` 之前**

```
POST /sessions
  ├─ capabilities::validate            061 的形状校验（工具名前缀、skill id、重名）
  ├─ capabilities::check_builtin_switch  ← 076 新增：名字必须在这个部署装配出来的表里
  │      判据 = state.template().tools（ToolTableSpec 五档）.build().declares(name)
  │      不在 → 400 `bad_request`，报文点名 + 附上这个部署实际有的那张名单
  ├─ capabilities::disabled_builtins   翻成 Vec<Arc<str>>（纯 agent_core 数据）
  └─ SessionTemplate::open_spec(id, path, host_tools, host_skills, disable_builtin)  ← 第五个参数
        actor::capabilities::assemble  restored ? session.disabled_builtins() : spec.disable_builtin
           → spec.tools.build().without_builtins(&disabled)   ← 减，排在 with_skills/with_host_tools 之前
        actor::capabilities::record    新建会话把开关 journaled 写一次（跟声明同一轮）
```

**天花板 = 五档，不含注入进来的东西**（这一条比任务书 §范围 3 更明确，如实记）：
宿主自己声明的 `web:`/`desk:` 工具关不掉，只在声明了 skill 时才出现的
`srv:skill/activate`/`deactivate` 也关不掉。理由不是漏了——那两样**已经完全由宿主自己
决定**（不想给就别报），再配第二个开关等于两个开关管同一件事，而且会造出「同一个名字
在两次请求里一次合法一次 400」（取决于这次带没带 `skills`）这种说不清的面。
`disable_builtin` 只减**宿主今天唯一控制不了的那批**——部署方定的那一档。

**为什么这一条必须报错**（069 §拍板的判据，两个相反的结论）：

| 位置 | 每轮跑吗 | 作者在不在场 | 结论 |
|---|---|---|---|
| 076 `check_builtin_switch` | 建会话那一次 | **在**（客户端还连着，能改） | **400 且点名** |
| 064 `skill_injection` 过滤 | **每轮** | 早不在了 | 绝不能报错，静默滤 |

`ToolTable::without_builtins` 对认不出的名字**静默跳过**，正是因为它落在下面那一格。

### 红线 9：`tool_table.rs` 300 → 251，按职责拆成两个新文件

075 之后它正好 300 行，零余量。拆的是**两件说得清、且不含「和」的事**，不是切后半截：

| 新文件 | 行 | 那一件事 |
|---|---|---|
| `tool_table_names.rs` | 89 | **名字规则**：`location_of`/`reversibility_of`——一个工具的全名怎么机械推出它那两个**不进 prompt** 的维度（为什么按名字推而不查表、为什么保守值必须是默认值） |
| `tool_table_disable.rs` | 72 | **关掉内置这件事**：076 的减法（`without_builtins`）——「不启用」为什么等于「连描述都不进 prompt」、为什么排在装配链最前、为什么不留一份 `disabled` 字段到渲染时再滤 |

于是 `tool_table.rs` 的一句话重新说得完：「五档装配 + `snapshot` 的三级判定 + `push_spec`
判重」。它旁边的四个 `#[path]` 子模块各是一件事（names / host / skill_tools / disable），
跟 062 的 `tool_table_host.rs`、064 的 `tool_table_skill.rs` 是同一条先例。

代价如实记：`tool_table_tests.rs` / `standard_local_tests.rs` 原来靠 `use super::*` 顺着
`tool_table.rs` 的导入白拿 `Location`/`SPAWN_TOOL` 等，现在各自点名 import（4 行 + 1 行）
——比为了让 `use super::*` 继续管用而在实现文件里留几个它自己用不上的导入干净。

### 子 agent：**`spawn` 一行都没改**

整棵 agent 树共用会话级这一份开关。落地上它是白拿的：工具表在 actor 起来时装配**一次**，
`Session::spawn_child` 从**父的表**里挑子集（028 的既有语义），父的表里没有的东西子也不
可能有。`Slot::DisabledBuiltins` 站队 `Upward`（跟 `HostTools`/`HostSkills`/`SkillsActive`
同一边）也是同一句话的状态面：它决定整棵树看得见什么，不是某一个 agent 的内部账本。

### 四条突变的真实红色输出（都已改回，`grep -rn "MUTATION" crates/` 无残留）

**① 关掉就真看不见** —— `actor/capabilities.rs` 不调 `without_builtins`：

```
test a_disabled_builtin_is_invisible_here_and_untouched_next_door ... FAILED
assertion `left == right` failed: 关掉的那两件不该在表里，没点名的一件不许少（顺序也不许变——红线 11）
  left: ["srv:fs/read","srv:fs/list","srv:shell/exec","srv:agent/spawn","srv:agent/status","srv:agent/collect"]
 right: ["srv:fs/read","srv:fs/list","srv:agent/status","srv:agent/collect"]

test guessing_a_disabled_tool_name_falls_through_to_unknown_tool ... FAILED
关掉的工具被模型硬猜到时该落 `unknown_tool`（跟任何不存在的工具一视同仁），实际：
[{"content":"test\n\n你是被分解出的子任务执行者：…","role":"system"},{"content":"随便干点什么","role":"user"}]
```

第二条红得特别有信息量：**那是子 agent 的 system prompt** ——过滤一去掉，模型猜的那个
`srv:agent/spawn` 当场真的开出了一棵子树。`declares()` 是 spawn 截获闸的唯一判据，
「不启用 ≠ 看得见但不给调」这句话在这里变成了一条会红的断言。

**② 只能减不能加** —— 路由把 `check_builtin_switch` 的结果丢掉（= 静默忽略）：

```
test an_unknown_name_is_rejected_by_name ... FAILED
assertion `left == right` failed: 拼错：该 400，实际 201 {"id":"bad-16","outcome":"created"}
  left: 201   right: 400
```

`201 created` 正是本仓最贵的那类形状：客户端以为关掉了，服务端愉快地建了会话，
模型照样调得到 `srv:shell/exec`，全程零报错。

**③ 恢复原模原样** —— `declaration()` 恢复那一支改用 `spec.disable_builtin`
（= 开关根本没落盘的等价形状）：

```
test a_recovered_session_keeps_its_switch_without_being_told_again ... FAILED
恢复出来的会话该带回它自己当初那份**减过的**表——开关没落盘的话这里会凭空多出两件：
["srv:fs/read","srv:fs/list","srv:shell/exec","srv:agent/spawn","srv:agent/status","srv:agent/collect"]
```

**④ 红线 11 确定性** —— `value/str_set.rs` 删掉 `sort()`/`dedup()`，独测 5 条**全红**：

```
test the_stored_bytes_do_not_depend_on_input_order_or_duplicates ... FAILED
assertion `left == right` failed: 倒序：关闭列表的输入顺序/重复项漏进了会话状态的落盘字节（红线 11）
  left:  "[\"srv:shell/exec\",\"srv:fs/list\",\"srv:agent/spawn\"]"
 right: "[\"srv:agent/spawn\",\"srv:fs/list\",\"srv:shell/exec\"]"

test a_snapshot_with_the_switch_restores_every_name ... FAILED
  left: ["srv:shell/exec","srv:agent/spawn","srv:fs/list","srv:shell/exec"]   ← 重复项也留下了
 right: ["srv:agent/spawn","srv:fs/list","srv:shell/exec"]
```

**这一条要如实说清它红在哪一侧**：删掉排序红的是**落盘字节**那一侧，不是 prompt 那一侧。
prompt 那边对关闭列表的顺序**天生免疫**——剔除是集合运算（`without_builtins` 用 `retain`
保住五档原有次序），换个顺序、多写一个重复项，出来的表一模一样。这是**更强**的性质，
不是漏测：`disabled_builtins_never_reach_the_prompt.rs::shuffling_the_switch_never_moves_a_byte`
把这个性质本身钉成断言（三家 provider × wire 字节 + 前缀镜像 + `drift`），它看住的是
「有人把 `retain` 换成顺序敏感写法」，跟 `sort()` 各钉各的落点、不重复。
`sort()` 真正的读者是**恢复时回放的那份字节**：两条本该一模一样的会话日志逐字节不同，
不报错。

### 「不带这个字段时逐字节不变」是怎么被钉住的

三个落点，一层比一层靠近 prompt：

1. `tool_table_disable_tests.rs::an_empty_switch_changes_nothing` —— 四档各跑一遍，
   `without_builtins(&[])` 出来的名字序列与压根没调过它**完全相同**；
2. `disabled_builtins_never_reach_the_prompt.rs::without_the_field_every_byte_is_what_it_was_before_076`
   —— **三家 provider** 各跑一遍，比的是请求体里 `tools` 那一段的**原始字节**（不是解析
   回来再序列化一次，那样切法本身就会把顺序问题洗掉）+ 前缀镜像的 `bytes`/`hash` +
   `drift != Some(Segment::Tools)`；
3. `http_capabilities_disable_builtin.rs` 里那个 `plain` 会话 —— 端到端、同一个 server
   进程，断言假上游收到的 `tools` 名字序列**是完整的一档**。它同时是「开关没粘在全局
   `SessionTemplate` 上」的证明（A 客户端关掉的工具 B 客户端也没了，会是最难查的那种）。

命名上也留了同一条承诺：`ToolTable::without_builtins` 空列表**提前 return**，`record` 对
空开关整段不做（连 `begin_turn` 那一下也不做），所以不带这个字段的会话**连一条 entry
都不多**、turn 号也不动，会话文件跟 076 之前逐字节相同。

### 新增测试

| 文件 | 条 | 钉什么 |
|---|---|---|
| `agent-core/src/command/disabled_builtins.rs`（内嵌） | 2 | journaled + undo/redo + 排序去重；空开关不落幽灵 entry |
| `agent-core/tests/disabled_builtins_indep_restore.rs` | 5 | 恢复路径（含 serde 往返逐字节、游标在开关之前**带正对照**、跟声明同时回来、落盘字节不随输入顺序漂） |
| `agent-runtime/src/tool_table_disable_tests.rs` | 6 | 剔除的语义（`declares()` 为假、幸存者保序、空开关空操作、认不出的静默跳过、集合语义、可逆性映射同步剔） |
| `agent-runtime/tests/disabled_builtins_never_reach_the_prompt.rs` | 4 | **进 prompt 的字节**（名字**和描述**都不在、打乱顺序字节不变、空开关字节不变、关表尾时共有那一段仍是字节前缀）×三家 |
| `agent-server/src/http/capabilities/builtin_switch.rs`（内嵌） | 6 | 天花板校验（拼错点名、别的档有的照拒、注入的不在天花板里、空开关放行、翻译原样） |
| `agent-server/tests/http_capabilities_disable_builtin.rs` | 3 | 端到端：看不见 + 隔壁不受影响、400 点名且不留会话、硬猜落 `unknown_tool` |
| `agent-server/tests/http_capabilities_disable_builtin_survive_restart.rs` | 2 | 恢复逐字节相同、有历史再带 → 400 `session_has_history`（带「不带就 200」的正对照） |

### 被这个槽位顶红、如实改掉的既有断言

`Slot::ALL` 13 → 14：`session_state.rs`（13 → 14）、`session_indep_snapshot_shape.rs`
（`EXPECTED_SLOT_COUNT` 13 → 14）、`subagent_indep_snapshot.rs`（39 → 42）、
`subagent_indep_despawn.rs`（每 agent 13 → 14、逐出 12 → 13、按需重建 13 → 14）、
`subagent_indep_tombstone.rs`（13 → 14 / 逐出 12 → 13）、`subagent_indep_undo_spawn.rs`
（13 → 14）、`subagent_indep_visibility.rs` 与 `graph/visibility.rs` 的 `Upward` 名单
（加 `DisabledBuiltins`）、`session_indep_accounting.rs` 的穷举 match。跟 073/064 一样：
**这些红是这些测试的价值本身**，改数字前逐条问过一遍「这个槽位该不该进快照」——
答案是必须，而且这一路的方向相反：不进快照 = 恢复出来的会话把当初藏起来的工具又端给
模型看，而那段历史里从没出现过它们。

### 收工验证（前台跑完，独立 `CARGO_TARGET_DIR`）

```
cargo test --workspace                → 1537 passed / 0 failed（基线 1509，+28 条新测试）
cargo test -p agent-server --features ts
  → 全绿。**协议面确实变了**：`Capabilities` 多一个 `disable_builtin?: Array<string>`，
    `packages/protocol/src/generated/Capabilities.ts` 已用
    `cargo run -p agent-server --features ts --example gen_protocol_ts` 同一次重生成，
    一致性检查通过（`packages/web/` 一个字没改——前端要不要出这个开关不在本 issue）。
cargo clippy --workspace --all-targets -- -D warnings   → Finished，零 warning
bash scripts/check-invariants.sh --all                  → 红线检查通过
```

### 三个如实记下的边角

1. **纯内存会话（没有 `default_sessions_dir`）沿用 073 的行为**：`existing` 那一支根本不
   `open_spec`，这次的开关被忽略。它们没有历史可复刻，真正需要拒绝的是「有历史」。
2. **`disable_builtin` 的名字带 `srv:` 前缀是对的**，跟 `capabilities.tools` 那个字段的
   规则（必须 `web:`/`desk:`）**正相反**——那里给的是「我有一个工具」，这里给的是
   「把你那个工具关掉」。两条规则各自的校验函数也是分开的两处，没有共用一条正则。
3. **关掉一件工具不会顺带关掉它的截获逻辑**：`dispatch` 的 spawn/skill 截获闸问的是
   `ToolTable::declares`，剔除之后它为假，所以模型硬猜那个名字会跟任何不存在的工具一样落
   `unknown_tool`（有端到端断言）。反过来说，**这个开关关不掉「工具存在但不给调」这种
   语义**——本 issue 明确不做那一档（用户拍板：不启用 = 模型看不见）。
