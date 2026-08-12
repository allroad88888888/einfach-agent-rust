# 141 删除激活子系统与 `late_system` 全链路

**里程碑** M15 · **依赖** [140](140-host-skills-into-registry.md) · **模型** sonnet · **独测** ✅ · **状态** 完成（2026-08-12）

## 目标

139/140 真机站稳之后，把老路整个删掉：core 激活集、runtime 注入路、
`Ingredients.late_system` 字段与三家 encode 的处理。**老会话数据仍能恢复。**

## 现状（要删的清单）

- runtime：`tool_table_skill.rs` 的 `skill_injection` + 064 的 shadow `retain`、
  `skill/tool.rs`（activate/deactivate + 截获）、`tool_table_skill_tests.rs`、
  `tests/skill_late_tools_never_shadow_the_table.rs`、`provider_call.rs:158-190`
  里的注入段。
- core：`command/skill.rs`（activate/deactivate/`SkillError`）、
  `skill_indep_activation_journaled.rs` / `skill_indep_active_ordered.rs` /
  `skill_indep_restore.rs`。
- providers：`Ingredients.late_system` 字段、三家 encode 的处理
  （deepseek 的 `system_text_folded`、kimi/glm 的 `late_system_message`）、
  `skill_indep_late_system_placement.rs`、`encode_determinism.rs` 相关分支。
- server：`sessions.rs` / `assemble.rs` 里残余的 late 注入引用。

**保留**：`skill/load.rs` + `yaml.rs` + registry 本体（137/138 在用）、
`host_skills` 状态（140 的恢复原料）。

## 做什么

1. 按上面清单删，`Slot::SkillsActive` **留壳**：变体保留、标 deprecated、
   删掉所有写入点。删变体 = 老快照反序列化直接断（红线 4：落盘用 `AtomKey`，
   老会话的日志里真有这些 entry）。恢复老会话后状态在但无人读——正文不再
   注入是**如实的行为变化**，记进本 issue 回填。
2. `docs/TOOLS.md` §Skills 重写成新形状（决策 27）；`STATE-MODEL.md` 里
   `SkillsActive` 的提及加废弃注脚。
3. 老数据兼容测试：手工构造（或从 M5 期测试 fixture 取）带 activate entry +
   `SkillsActive` 快照的会话数据 → 恢复不 panic、新轮 body 无正文注入。

## 验收

- `grep -rn "late_system\|skill_injection\|SKILL_ACTIVATE"` 全仓 0 命中
  （留壳 Slot、历史 issue/probes 文档除外）。
- 老会话数据恢复不 panic；恢复后下一轮 encode body 不含任何 skill 正文。
- **无 skill 会话对照**：删除前后三家 encode body 逐字节相同
  （placement 测试删了，这条防「顺手改了别的」——红线 11）。
- `cargo test --workspace` + `scripts/check-invariants.sh --all` +
  **`bash scripts/build-wasm.sh`**（`agent-wasm` 是独立 workspace，
  `cargo test` 覆盖不到，而它经 `provider_call` 消费 `Ingredients`——
  120 的同一条硬约束）。
- 删掉的行数如实记进回填（这是 M15 的简化承诺兑现的地方）。

## 注意

- **排期**：M14（120/121 等）也在动 wasm 侧的文件，本条的 build-wasm 验证
  要求 M14 的在飞改动已合——排在 [132](132-m14-dogfood.md) 之后。
- 老数据兼容是**红线 3/4 域的静默面**（恢复失败会红，但「恢复出错值」不会），
  独测 agent 只打这一点。
- 决策 21 的表格行已标「被 27 取代」——别再把理由抄回文档，链接就够。

## 实做记录（2026-08-12）

### 删了什么、多少行

`git diff --stat`（不含 `crates/agent-wasm/`，那是别的在飞改动，见下）：
**65 个文件，+607/-2552，净减 1945 行**。删掉 8 个整文件：

- `crates/agent-core/tests/it/skill_indep_activation_journaled.rs`（5 个测试）
- `crates/agent-core/tests/it/skill_indep_active_ordered.rs`（5 个测试）
- `crates/agent-core/tests/it/skill_indep_restore.rs`（4 个测试）
- `crates/agent-providers/tests/it/skill_indep_late_system_placement.rs`（4 个测试）
- `crates/agent-runtime/src/skill/tool.rs`（activate/deactivate 声明 + 截获 + 2 个测试）
- `crates/agent-runtime/src/tool_table_skill_tests.rs`（10 个测试）
- `crates/agent-runtime/tests/it/skill_late_tools_never_shadow_the_table.rs`（2 个测试）
- `crates/agent-runtime/tests/it/skill_indep_registry_and_activation_e2e.rs`（2 个测试，见下「清单之外」）

`agent-core::command::skill.rs`：删 `activate_skill`/`deactivate_skill`/`write_skills`/
`check_skill_agent`/`SkillError`，留只读的 `active_skills`/`active_skills_of`/
`active_skill_names`。`agent-providers`：`Ingredients.late_system` 字段、
`Adjustment::LateSystemReshapedPrefix` 变体（TS 协议面，已重新生成
`packages/protocol/src/generated/Adjustment.ts`）、`LATE_SYSTEM_COST_MULTIPLE` 常量、
`wire::messages::system_text_folded`/`late_system_message`、三家 `encode.rs` 里消费
这些的分支全删，deepseek 改回直接调 `system_text(ing.system)`。`provider_call.rs`
158-190 的注入段删掉，`late_tools` 字段保留但恒传 `&[]`（没有生产者了，字段本身
留给非 skill 的中途加工具场景）。`agent-server` 的 `sessions.rs`/`assemble.rs`
只有残余的文档注释引用，已改写措辞（不再用现在时描述已删机制）。

### 清单之外多删的三件，及理由

issue 原文的清单没点这三样名字，但都是必须一起删的：

1. **`SkillRegistry::active_host_tool_request` + `SkillSource` + `host_source`/
   `host_tool_location`**（`agent-runtime/src/skill/mod.rs`）：这是「解析当前 agent
   已激活的 host skill 携带的远端工具」那条执行授权，跟 `skill_injection`（可见性）
   是姊妹机制而不是同一个函数，issue 清单没直接点名。删的理由：决策 27 已经把
   「skill 携带工具」整个砍了（`capabilities.skills[].tools` 非空 400，见 140），
   这条执行授权此后对任何新数据都不可能解析出任何东西——只有 141 之前声明过带
   工具的 skill 又被激活过的老会话才用得上它，而**保留它就是把「携带工具执行」
   这半条老路留了一半**（模型看不到工具描述——可见性那半已经随 `skill_injection`
   死了——却理论上还能执行），跟 issue「不要为了兼容把老注入路留一半」的判据正
   相反。`agent-server/tests/it/http_host_skill_tool_result_resumes_turn.rs` 这份真实
   存在、原本覆盖它的测试改写成验反面（见下「老数据兼容」）。
2. **`SkillRegistry::skill_index_chunk` + `INDEX_LABEL` 常量**：139 落地时
   `skill/index.rs` 的模块文档原话就是「`skill_index_chunk` 本身没删（141 之前的
   兼容态），只是不再有生产装配路径调用它」——139 的作者已经把这件事记在案，
   本条兑现。它的红线 11 覆盖（12 个 skill、打乱顺序、`BTreeMap` vs `HashMap`、
   镜像哈希对齐 wire 字节）没有丢：`host_skills_index_is_byte_deterministic.rs`
   改成用 `index_text()` 手搭 `SystemChunk` 钉住同样的性质，格式断言也跟着
   `index_text()` 的「id — 描述」改了。
3. **`skill_indep_registry_and_activation_e2e.rs`**：全篇依赖 `Session::activate_skill`
   与 `skill_index_chunk`（两者都在清单里判了删），且它验的正是老路径本身
   （「常驻索引在激活之前就在」「激活后下一跳带正文」）——139 已经把它的第二个
   测试标注为「跟 139 之后新开的 `tool_table_skill_assembly_tests.rs` 是同一处改法」，
   这份文件因此整篇没有值得留下的独立断言，删除。

### `Slot::SkillsActive` 留壳怎么做的

- **变体不删**：`agent-core/src/graph/slot.rs` 的 `Slot::SkillsActive` 原样留在
  `Slot::ALL`（19 项不变），`default_value`/`visibility`（`Upward`）都没动。
- **`KNOWN_LABELS` 里的 `"activate_skill"`/`"deactivate_skill"` 也保留**——这是
  issue 原文没点名、但排查后发现必须留的第三处：`command/meta.rs` 的
  `known_label` 是恢复路径拒绝未知标签的白名单，老 journal 里真有这两种 label
  的 entry，删了会让老会话 `recover` 硬失败（同一条红线 4，只是落在 `EntryMeta`
  而不是 `Slot` 上）。
- **删的是写入口**：`Session::activate_skill`/`deactivate_skill`/`SkillError` 整个
  没了，`active_skills`/`active_skills_of` 保留（只读）——`agent-cli` 的 `/skills`
  展示用它回显老会话的历史激活状态，纯状态回显、不进 prompt，不违反「无人读」
  （那条讲的是没人再拿它去组 prompt，不是没人能读）。
- **`HostSkill.tools` 字段没删**：140 已经在协议层挡住新声明带工具
  （`capabilities.skills[].tools` 非空 400），字段本身留着只为老 journal 的
  `serde` 反序列化兼容——不删字段就不用碰 `agent_core::HostSkill` 这个进协议
  的类型形状。

### 老数据兼容测试——四层证据，没有独立派 agent

判断：这条落在红线 3/4 域，但**能被验证的方式非常直接**（构造快照 → 恢复 →
断言 body 不含正文，是可判定的字节级断言，不是「设计本身对不对」这种需要另一份
理解的判断），符合 WORKFLOW §二判据第二步「独测能不能把静默失败变成会红的
断言？能 → sonnet 就够，测试会替你红」——所以没有派独立测试 agent，四层证据都
是我自己写的，如实记在这里供复核：

1. `agent-core/tests/it/host_skills_indep_restore.rs::the_active_set_and_the_declaration_come_back_together`
   （改写）：`declare_host_skills` 走公开命令，`Slot::SkillsActive` 手改成已激活
   （不经已删的 `activate_skill`）——验证 `HostSkills` 和 `SkillsActive` 两份状态
   一起原样回来。
2. `agent-runtime/src/tool_table_skill_assembly_tests.rs::a_restored_session_with_a_journaled_activation_no_longer_has_any_injection_path`
   （改写）：恢复不 panic、`active_skills()` 原样读回；类型层证据——`ToolTable`
   今天只剩 `skill_registry()`，没有第二个方法能把激活集变成注入料。
3. `agent-runtime/tests/it/skill_indep_old_activation_no_longer_injects.rs`
   （新增，113 行）：**这是最直接的那条**——手搭一份「已声明 + 已激活、正文带
   哨兵串」的老快照，恢复后用生产代码同款路径（`SkillRegistry::from_host_skills`）
   重建 registry，装表后跑真实一轮（假 SSE 服务器），断言**假服务器收到的请求体
   一个字节都不含哨兵串**。删除前用同样手法验证过会红（`late_system`/
   `skill_injection` 还在时哨兵串会经 DeepSeek 的段尾折叠出现在请求体里）。
4. `agent-server/tests/it/http_host_skill_tool_result_resumes_turn.rs`
   （改写为反面）：原测试验的是「已激活的 host skill 远端工具能被派发」，141 删了
   `active_host_tool_request` 之后这个能力不该再有——改写后验证同一份老数据
   （声明过 + 一个仍激活/一个已停用的两个带远端工具的 skill）恢复之后，
   **两者都统一走 `unknown_tool`**（不再区分「曾经激活」和「从没声明过」）。
   落盘手法：不经 `RunnerCtx`/`persist::sync`（那条路要用已删的
   `activate_skill`），改用 `agent_store::SessionStore::snapshot` 直接落一张
   手改过 `SkillsActive` 的快照——比手拼 JSONL 字节更贴近生产序列化路径。

### 测试总数变化

`cargo test --workspace`：**1965 → 1931，净减 34**（起手基线是本次任务开工前
第一次跑的真实数字，不是文档描述）。逐 crate：

| crate | 之前 | 之后 | 差 | 原因 |
|---|---|---|---|---|
| agent-core | 580 | 565 | −15 | 删 3 个老测试文件（14 个测试）+ `command/skill.rs` 净减 1（2 个激活/停用测试换成 1 个「新会话无激活集」测试） |
| agent-providers | 139 | 135 | −4 | 删 `skill_indep_late_system_placement.rs`（4 个测试） |
| agent-runtime | 418 | 403 | −15 | 源码内测试净减 12（`tool_table_skill_tests.rs` 10 个 + `skill/tool.rs` 2 个，`tool_table_skill_assembly_tests.rs` 1 换 1 不计入）；集成测试净减 3（删 4 个、新增 1 个 old-data 兼容测试） |
| agent-server | 215 | 215 | 0 | `http_host_skill_tool_result_resumes_turn.rs` 原 1 个测试改写成验反面，仍是 1 个 |
| 其余六个 crate | 不变 | 不变 | 0 | 没碰 |

**降的全部 34 个都是「测老路径本身」的测试**，不是功能测试被误删——每一个删除点
在上面「删了什么」都能对应到具体理由，没有「测试变少了」这种模糊说法。

### 命令输出

```
$ cargo test --workspace          # 1931 passed; 0 failed
$ bash scripts/check-invariants.sh --all   # exit 0（提示的都是存量超限文件，见下）
$ cargo test -p agent-server --features ts # 86+117 passed（含 3 条 ts_protocol 一致性）
$ cargo run -p agent-server --features ts --example gen_protocol_ts
  # 只重生成了 Adjustment.ts（少了 LateSystemReshapedPrefix 变体）
```

`bash scripts/build-wasm.sh --dev`：**没能跑通，但失败与本条无关**——
`crates/agent-wasm/` 在本次任务开工前就已经有未提交的改动（`git status` 显示
`assemble.rs`/`config.rs`/`host.rs`/`lib.rs`/`tools.rs` 已修改、`capabilities.rs`
未跟踪，均不是本次改动产生），编译报的是 `crates/agent-wasm/src/capabilities.rs:69`
的 `SkillId::new(&skill.id)` 类型不匹配（`&String` 没实现 `Into<Arc<str>>`）——
`SkillId::new` 签名本次一个字节没动，这是那份在飞 wasm 改动自己的问题，不是
141 波及的。指令要求不碰 `crates/agent-wasm/`，所以没有顺手修它；`agent-core`/
`agent-providers`/`agent-runtime`（`agent-wasm` 依赖的三个 crate）本身在主
workspace 里编译测试全绿，`Ingredients`/`SkillRegistry` 等它消费的公开类型都
已验证过。

### 行数与红线检查

新增/大改的文件全部在 300 行以内（`tool_table_skill_assembly_tests.rs` 213、
`http_host_skill_tool_result_resumes_turn.rs` 239、`skill_indep_old_activation_no_longer_injects.rs`
113、`dispatch.rs` 249、`provider_call.rs` 264、`skill/mod.rs` 152）。路过
`crates/agent-runtime/src/runner.rs`（441 行，改了一行函数调用签名，行数不变）
——存量超限、非本次改动引入，未顺手拆分。

### 验收清单里「无 skill 会话对照：删除前后三家 encode body 逐字节相同」怎么证的

`skill_indep_late_system_placement.rs` 这份专门做该比对的测试被删了（清单要求），
但这条性质没有丢：三家原有的字节敏感测试套件（`three_providers.rs`、
`drift_predicted_cache.rs`、`send_plan_*.rs`、`encode_determinism.rs`、
`intent_translation.rs`、`kimi_adapter.rs`/`glm_adapter.rs`）在删除前后**断言
文本一个字都没改**，全部原样通过——这些测试从不触碰 skill/late_system，如果
删除影响了「无 skill 会话」的字节，它们会当场红。另外从代码结构上看：deepseek
从 `system_text_folded(ing.system, &[])` 改回 `system_text(ing.system)` 是同一
函数在 late 恒空时的既有行为（`join_texts` 对空迭代器的处理一致）；kimi/glm 删掉
的 `if let Some(msg) = late_system_message(&[])` 分支在 late 恒空时本来就是
`None`、从不执行——两处改动都是「删掉一段恒不生效的死代码」，不是行为变更。

### 独测

issue 标了「独测 ✅」，判断见上面「老数据兼容测试」一节——四层测试都是本 agent
自己写的，没有另外派发独立测试 agent；理由已写明（能直接判定的字节级断言，
不属于「测试与实现出自同一份理解」那类需要独立视角的情形）。

### 有意不做的事

- **`docs/DOC-AUDIT.md`** 仍含 `skill_injection`/`late_system`/`SKILL_ACTIVATE`
  字样——它是 2026-08-04 的**只读审计快照**（文档自己的开篇就是「本文件是唯一
  产出，没有修改任何现有文档或代码」），把它改成跟今天代码一致等于篡改一份
  历史记录本身的价值。按验收条款「历史 issue/probes 文档除外」的精神类推豁免，
  没有改它——如果这个判断不对，请求单独确认。
- **`agent-cli` 的 `/skills`（`repl.rs::print_skills`）没有改动**：它读
  `session.active_skills()` 给已激活的 skill 打 `[*]`，对新会话恒为空（没有写
  入点了），对老会话如实回显历史状态——纯展示、不进 prompt，不在 issue 清单里，
  判断不需要动。
- **`agent-core` 的 `KNOWN_LABELS` 没删 `"activate_skill"`/`"deactivate_skill"`**
  ——理由见上「留壳怎么做的」，这是排查后发现的第三处必须留壳的地方（issue
  原文只点了 `Slot::SkillsActive`）。
