# 064 `capabilities.skills` + 唤醒 server 形态的 skill 机制

**里程碑** M10 · **依赖** 062 · **模型** sonnet · **独测** ✅（红线 11：索引顺序）

注入的第二类。顺带补一个勘查发现的空洞。接缝见
[HOST-CAPABILITIES.md](../HOST-CAPABILITIES.md) §八。

## 背景：server 形态下 skill 是休眠的

`ToolTableSpec` 的**五档全都不接 `.with_skills(..)`**，`SkillRegistry::load` 在
`agent-server` / `agent-server-bin` / Tauri 桌面壳里**一次都没被调用过**——只有 `agent-cli` 调。
所以**经 HTTP 起的会话，`srv:skill/activate` 根本不在表里**：M5 做的整套 skill 机制，
在 server 形态下等于不存在。本 issue 顺带唤醒它。

## 范围

1. **注入的 skill 进 per-session `SkillRegistry`**（`BTreeMap`，有序白拿）：
   `description` 进**常驻索引**（一行一个）；`body` 与自带 `tools` **等模型
   `srv:skill/activate` 才进这一轮**（`late_system`/`late_tools`）——**延迟加载是既有机制，
   一行都不要新造**。
2. **registry 非空 → 工具表接 `.with_skills(..)`**，于是 `srv:skill/activate` /
   `srv:skill/deactivate` 出现在这个会话的表里。
3. **顺带决定**（建议一并做，但要在记录里说明理由）：server 是否也从本地 `./skills/` 装载。
   `SkillRegistry::load(dirs)` 本来就收任意目录列表，两者进同一个 registry。
4. **skill 自带工具的既有坑**（如实处理，别放大）：`late_tools` **不进 `declares()`**，
   不会被任何截获路捕获。所以注入的 skill 自带工具**必须**同样是 `web:`/`desk:` 前缀
   （061 已校验）。**本 issue 只需确认校验覆盖到了**，不要去重构 `late_tools` 的既有行为。
5. **跨路径撞名 —— [069](069-name-collision-policy.md) 已拍板，本 issue 落地它**：
   宿主注入的 `web:foo`（进工具表）撞上某个 skill 激活时 `late_tools` 里的 `web:foo`
   时，**表赢，多余那份在 `ToolTable::skill_injection` 就滤掉**。
   赢家不是选出来的——`declares()` 为真是因为**表**里有它，远端第五路把调用派给宿主
   注册的那一份，skill 带的那份从来没有过自己的执行路径；滤掉它**执行侧一个字节不变**，
   只是不再给模型看一份它影响不了的 schema（069 §拍板 第 2 问）。
   **这里绝不能报错**：`skill_injection` 每轮都跑，作者早就不在场了。
   ⚠️ 这一条动的是 `tool_table.rs` 的 `skill_injection`，**必须等 062 落地之后**。
6. **server 要不要也从磁盘 `./skills/` 装载**（就是上面第 3 条）——069 的**推荐是不装载**：
   宿主已经有声明入口，两个来源合流只会造出「同一份请求在不同部署上行为不同」的面。
   **若仍要开**，则宿主声明的 skill id 撞上磁盘已装载的 id → **400**（跟 061 同一条闸，
   那一刻客户端还在线）；**不许**静默让磁盘那份盖掉宿主声明的那份。

## 验收（可判定）

- 注入两个 skill → 该会话 system 段出现**索引两行**（`id: 描述`），**`body` 不在**。
- 模型 `srv:skill/activate` 其中一个 → 那一轮 `late_system` 出现它的 `body`、
  `late_tools` 出现它自带的工具；**另一个仍然只有索引行**。
- `/undo` 撤掉激活 → 下一轮 `body` 消失（journaled，既有机制白拿）。
- **作用域**：另起一个不带声明的会话 → `srv:skill/activate` **不在表里**。
- **红线 11**：同一份 skill 声明两次渲染索引字节相同；**打乱数组顺序字节仍相同**
  （`BTreeMap` 给一半，另一半是「数组顺序不能泄漏」）；删掉排序 → 红（突变验证，贴输出）。
- **跨路径撞名（069）**：表里有 `web:foo` + 某 skill 也带 `web:foo` → 激活它那一轮的
  `late_tools` 里**没有** `web:foo`（表那份还在），且 `late_system` 里它的正文**一个字节
  不少**（滤的是工具不是 skill）。删掉过滤 → 红。

## 注意

- **不要重构 skill 的既有注入路**：`late_tools` 在三家 adapter 的合并点各不相同
  （DeepSeek/GLM 合进顶层 tools 并报 `Adjustment`，Kimi 走消息级）。本 issue 只是让
  registry 非空，下游一行不改。
- `body` 的**长度上限**属安全那节（HOST-CAPABILITIES §八，**暂缓**），本 issue 不做，
  但**如实在记录里写明「现在没有上限」**。
- **不要碰** `crates/agent-tools/`。
- 红线 9：≤300 行。
- 收工验证前台跑完（WORKFLOW §四 -1）。

## 实做记录（完成 · 2026-08-04）

### 两个要拍的，先说结论

| 要拍的 | 拍了什么 | 一句话理由 |
|---|---|---|
| **skill 声明进不进 store** | **进**（`Slot::HostSkills`，journaled，跟 073 的 `HostTools` 同构） | 不进就是一份**指向空 registry 的激活集**，而且 073 之后宿主根本没有第二次机会报 |
| **要不要也从磁盘 `./skills/` 装载** | **不装载**（069 的推荐，采纳） | 两个来源合流会让部署者改一改 `./skills/` 就悄悄改写一段**历史对话**该长什么样 |

#### 为什么 skill 声明**必须**进 store（比 073 的工具那一路更硬）

073 那三条（历史对话是在那一份表下产生的 / 红线 11 前缀 / 恢复是忠实重放）对 skill 原样
成立——skill 的**索引行进 system 段**，同样是稳定前缀的一部分。另外两条是 skill 独有的：

1. **`Slot::SkillsActive` 早就在 store 里了**（039）。声明不落盘，恢复出来就是一份**悬空
   引用**：会话状态说 `crm-flow` 激活着，`SkillRegistry::injection` 却查不到这个 id
   （**静默跳过**是它的既有语义），而模型的历史里明明写着「我激活了它、读过那段正文」。
   状态自洽、功能不报错、模型看到的东西前后矛盾——本仓最怕的那类静默错值。
   独测 `host_skills_indep_restore.rs::the_active_set_and_the_declaration_come_back_together`
   就是钉这一条的。
2. **宿主没有第二次机会报。** 073 落地之后，有历史的会话再带 `capabilities` 一律
   **400 `session_has_history`**。不存下来 = **永久没了**，连「重连时重报一遍」这条（已被
   用户否决的）退路都不存在。也就是说：062 当年那个中间态（「这次请求要是又带了声明，
   它会跟 created 走同一条 open 装进去」）**在 073 之后已经不复存在**——不存就是彻底丢，
   没有中间态可留。

代价如实记：`Slot::ALL` 12 → 13，于是「每个 agent 几个槽位」的既有断言集体要改
（见下「被顶红的槽位计数」）。**这些红是这些测试的价值本身**，改数字前逐条问过一遍。

#### 为什么**不**从磁盘 `./skills/` 装载

069 §拍板给的理由（宿主已经有声明入口；两个来源合流会造出「同一份请求在不同部署上行为
不同」的面）成立，而且 **073 之后它比写下来时更硬**：宿主声明现在是**会话状态**
（journaled，恢复时逐字节复刻），磁盘上那份不是——开了合流，部署者改一下 `./skills/` 就能
悄悄改写一段**历史对话**该长什么样，正好是 073 刚堵上的那个洞。

落地形状把这条决定钉进了类型：`SkillRegistry::from_host_skills` 是**构造器**，不是能接在
`load(dirs)` 后面的 `self` builder。想合流的人必须显式加一条合并路径，那时才轮得到 064 §6
那条闸（宿主声明的 id 撞磁盘已装载的 id → 400）；**绝不会**有人不小心让 `BTreeMap::insert`
把它变成静默的后来居上。`agent-cli` 那条磁盘装载路一个字节没动。

### 改动面

```
POST /sessions  {"capabilities":{"skills":[…]}}
  │
  ├─ capabilities::validate         061 的纯函数（skill id 白名单 + 自带工具同一条校验）
  ├─ capabilities::host_skills      064 新增：声明 → Vec<HostSkill>（纯 agent_core 数据）
  ├─ SessionTemplate::open_spec(id, path, host_tools, host_skills)   ← 第四个参数
  └─ actor::capabilities::assemble  ← 064 新文件：声明从哪来 → ToolTable + system 段
         restored ? session.host_skills() : spec.host_skills
         → SkillRegistry::from_host_skills → 非空才 .with_skills(..)
         → system.push(registry.skill_index_chunk())
     actor::capabilities::record    新建会话把声明 journaled 写一次（自成一轮）
```

| 文件 | 行 | 干什么 |
|---|---|---|
| `crates/agent-core/src/value/host_skills.rs` | 191 | **新**：`HostSkill` 形状 + ↔ `AgentValue::Json` 的唯一一处编解码（按 id 排序）+ 4 条单测 |
| `crates/agent-core/src/command/host_skills.rs` | 126 | **新**：`declare_host_skills` / `host_skills`（journaled 读写）+ 2 条单测 |
| `crates/agent-core/src/graph/slot.rs` | 234 | 加 `Slot::HostSkills` + 默认值 + `ALL`（12 → 13） |
| `crates/agent-core/src/graph/visibility.rs` | 166 | 站队 `Upward`（跟 `SkillsActive` 同一边，穷举 match 逼着回答） |
| `crates/agent-core/src/command/meta.rs` | 116 | `KNOWN_LABELS` 加 `declare_host_skills` |
| `crates/agent-runtime/src/skill/mod.rs` | 184 | **`SkillRegistry::from_host_skills`**（构造器，含「为什么不是 builder」） |
| `crates/agent-runtime/src/tool_table_skill.rs` | 90 | **新**：从 `tool_table.rs` 拆出「skill 这件事」+ **撞名过滤**（069 落地） |
| `crates/agent-runtime/src/tool_table_skill_tests.rs` | 121 | **新**：过滤的 5 条单测（含正对照、幂等、内置名同规则） |
| `crates/agent-runtime/src/tool_table.rs` | 278 | 拆出去 30 行（296 → 278），给 069 的 `push_spec` 留出余量 |
| `crates/agent-server/src/http/capabilities/assemble.rs` | 220 | 加 `host_skills` + 抽出共用的 `tool_spec`（顶层与 skill 自带的翻译只有一处）+ 3 条单测 |
| `crates/agent-server/src/http/capabilities/mod.rs` | 222 | 删掉 061 留在 `description`/`body` 上的两句 `#[allow(dead_code)]` |
| `crates/agent-server/src/actor/capabilities.rs` | 138 | **新**：从 `body.rs` 拆出「宿主注入的能力怎么落地」整件事（见下） |
| `crates/agent-server/src/actor/body.rs` | 257 | 313 → 257：三段挪进上面那个文件，只留两次调用 |
| `crates/agent-server/src/registry/spec.rs` | 183 | `OpenSpec` 加 `host_skills` |
| `crates/agent-server/src/http/config.rs` | 291 | `open_spec` 第四个参数 + 一条「注入的 skill 不粘在 template 上」的单测 |
| `crates/agent-server/src/http/routes/sessions.rs` | 271 | 校验之后翻译、当参数传下去（`session_has_history` 那道闸对 skill 一视同仁，一行都没改） |

**测试**：`agent-core/tests/host_skills_indep_restore.rs`（4 条）、
`agent-runtime/tests/host_skills_index_is_byte_deterministic.rs`（4 条，红线 11）、
`agent-runtime/tests/skill_late_tools_never_shadow_the_table.rs`（1 条端到端，069）、
`agent-server/tests/http_capabilities_skills.rs`（2 条：索引/作用域、激活）、
`agent-server/tests/http_capabilities_skills_survive_restart.rs`（2 条：恢复、undo）。

### 红线 9 顶破了两处，都按职责拆了

**`tool_table.rs` 296 → 278**：拆出去的是**「skill 这件事」**（registry 为什么归表拥有、
每轮怎么展开成注入料、撞名怎么滤），不是「后半截代码」。判据是「说得清它是干嘛的、且不含
『和』」——注入（`tool_table_host.rs`，062 的先例）和 skill 各自都有一整套自己的理由要写。
**给 069 留了位置**：那条还没做的活（`push_spec` + 撞名整条丢弃）动的是五档装配那一段，
仍然在 `tool_table.rs` 里，现在有 22 行余量；它跟 skill 那一段不再挤同一个文件。

**`body.rs` 313 → 257**：加完 skill 装配当场顶破 300（hook 当场报），于是把**宿主注入的
能力在这个会话里怎么落地**整件事拆进 `actor/capabilities.rs`——声明从哪来（073 的
`restored ? 历史 : 请求`）、怎么变成表和 system 段、什么时候 journaled 地写进历史，三条
issue（062/064/073）的结论从此挂在同一个文件的模块文档上。`body.rs` 那句「actor 线程跑
什么」因此重新说得完，而且**比本 issue 之前还短**（072 正在同一个文件里干活，这是顺带的
好处，不是理由）。

### skill 自带工具的 `reversibility`：**丢掉**，如实记一笔

061 的形状里 skill 自带的工具跟顶层工具是同一个类型，所以它也能写 `reversibility`。
`assemble::host_skills` **把它丢了**，落进 store 的也只有进 prompt 的那三个字段。

理由：`late_tools` 今天**连 `ToolTable::declares` 都不进**（069 §另记一笔 记的那个可执行性
洞——skill 自带的 `web:`/`desk:` 工具今天根本执行不了，落 `unknown_tool`），没有任何一处会
去查它的可逆性。翻译成一个没有读者的字段、再存进会话历史，只会给将来一个「它一直是对的」
的错觉。真要把 `late_tools` 接上执行，那时该定的是「激活时它进不进表」这个更大的问题
（进表就改前缀，红线 11），不是从历史里挖一个当年顺手存下的值。

**064 §范围 第 4 条要求的「确认校验覆盖到了」**：覆盖了——`validate::check_tool` 对顶层和
每个 skill 自带的工具跑同一条规则，且工具名唯一性是**全局**的（同一个 `BTreeSet`）。既有
测试 `tools_carried_by_a_skill_go_through_the_same_check` 钉住，本 issue 一行没改。

### `body` 长度上限：**现在没有**

`HostSkill.body` 今天**没有任何长度上限**（HOST-CAPABILITIES §九「这一节还没定的」最后
一条，属安全那一节，本 issue 明确不做）。一份很长的 `body` 会让**激活之后的每一轮**都变贵
——这是确定的成本，不是不确定的风险。已写进 `HostSkill.body` 的字段文档，免得下一个人以为
哪儿有闸。

### 两条突变的真实红色输出

**① 红线 11（索引顺序）——把 `SkillRegistry.skills` 的 `BTreeMap` 换成 `HashMap`
（= 删掉排序）**，4 条里红 3 条：

```
test the_index_is_one_sorted_line_per_skill_and_carries_no_body ... FAILED
assertion `left == right` failed: 索引该是按 id 排序、一行一个「id: 描述」（第一行是那句抬头）
  left: ["mail-flow: 发信流程", "audit-flow: 审计", "zeta-flow: 最后一个流程", …]
 right: ["alpha-flow: 第一个流程", "audit-flow: 审计", "beta-flow: 第二个流程", …]

test the_same_declaration_renders_the_very_same_index_twice ... FAILED
assertion `left == right` failed: deepseek：同一份 skill 声明两次渲染，前缀镜像的 System 段不一样
  left:  SegmentImage { segment: System, bytes: 413, hash: 8284132494547897260 }
 right:  SegmentImage { segment: System, bytes: 413, hash: 12171411687738759127 }

test shuffling_the_declaration_array_never_moves_a_byte_of_the_index ... FAILED
assertion `left != right` failed: deepseek/倒序：同一份 skill 声明换个数组顺序就被判成前缀漂了
                                  ——功能一切正常，只是每一轮都全价（红线 11）
  left: Some(System)   right: Some(System)
```

**`bytes: 413` 一模一样、`hash` 不同**——这正是 063 那条「只比长度不够」的范式在 skill 这
一路的现形：索引行换个顺序，长度一个字节不差，缓存却整条作废。

**② 跨路径撞名（069）——删掉 `skill_injection` 里那行 `late_tools.retain(...)`**，
端到端那条红：

```
test the_name_the_table_already_has_appears_exactly_once_and_the_skill_body_is_intact ... FAILED
assertion `left == right` failed: web:crm/close 在请求体里出现了 2 次——两份同名说明书一起进 prompt，
  模型按哪一份出参完全看它自己，而只有一份对得上真正会跑的那件事（069）
  left: 2   right: 1
```

请求体里真的并排躺着两条（这段是失败信息原样打出来的）：

```json
{"function":{"description":"宿主注册的那一份说明书 TABLE_SIDE_MARKER","name":"web_3Acrm_2Fclose",…}},
{"function":{"description":"skill 自带的那一份说明书 SKILL_SIDE_MARKER","name":"web_3Acrm_2Fclose",…}}
```

同一个突变下单测也红两条（**正对照那条仍然绿**，说明红的不是「全滤掉」）：

```
test tool_table::skill_tools::tests::a_name_the_table_already_has_never_enters_late_tools ... FAILED
  left: ["web:crm/close", "web:crm/extra"]   right: ["web:crm/extra"]
test tool_table::skill_tools::tests::a_builtin_name_is_filtered_by_the_same_rule ... FAILED
  left: ["srv:fs/read", "web:crm/extra"]     right: ["web:crm/extra"]
test tool_table::skill_tools::tests::without_a_collision_every_carried_tool_goes_through ... ok
```

两个突变都已改回，收工 `grep -rn "MUTATION" crates/` **无残留**。

**这条撞名今天走哪条路真够得着**：HTTP 那条**够不着**——061 把「顶层 tools 与每个 skill
自带的 tools」放在同一个集合里判唯一，同一份声明里的 `web:foo × web:foo` 直接 400。真够得着
的是 `agent-cli`：它从磁盘 `./skills/` 装载 skill，而一份 `SKILL.md` 完全可以声明一个跟内置
工具同名的工具（061 管不到磁盘）。所以单测里那条 `a_builtin_name_is_filtered_by_the_same_rule`
不是凑数——它才是今天的真实形状。过滤的判据是**「表里有没有」**，不是「是不是注入进来的」。

### 被这个槽位顶红、如实改掉的既有断言

`Slot::ALL` 12 → 13：`session_state.rs`（12 → 13）、`session_indep_snapshot_shape.rs`
（`EXPECTED_SLOT_COUNT` 12 → 13）、`subagent_indep_snapshot.rs`（36 → 39）、
`subagent_indep_despawn.rs` / `subagent_indep_tombstone.rs`（逐出 11 → 12）、
`subagent_indep_undo_spawn.rs`（12 → 13）、`subagent_indep_visibility.rs` 与
`graph/visibility.rs` 的 `Upward` 名单（加 `HostSkills`）、`session_indep_accounting.rs`
的穷举 match。跟 073 一样：**这些红是这些测试的价值本身**。

### 踩到的一个坑（留着防止再犯）

端到端测试里「等这一轮跑完」不能靠**一条 SSE 终态帧**：`GET /events` 会补发环形缓冲里的
历史帧，**上一轮**的终态会被当场读到，于是「等这一轮结束」变成「立刻返回」，症状是紧接着
读请求体时 `swap_remove index (is 2) should be < len (is 2)`。判据改成**假上游收到的请求
数**。已写进 `http_capabilities_skills_survive_restart.rs` 的 `wait_for` 注释。

### 收工验证（前台跑完，独立 `CARGO_TARGET_DIR`）

```
cargo test -p agent-core -p agent-server -p agent-runtime
  → 146 个测试二进制，788 passed / 0 failed（含并发会话此刻在这几个 crate 里的测试）

cargo test -p agent-server --features ts
  → 全绿（本 issue **没有改协议类型**：`CapabilitySkill` 的形状 061 就定好了，
    064 只是给它的字段接上读者，`generated/` 一个文件都不用重生成）

cargo clippy -p agent-core -p agent-runtime -p agent-server -p agent-cli --all-targets -- -D warnings
  → Finished，零 warning

bash scripts/check-invariants.sh --all → 红线检查通过
```
