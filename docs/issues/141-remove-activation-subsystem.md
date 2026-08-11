# 141 删除激活子系统与 `late_system` 全链路

**里程碑** M15 · **依赖** [140](140-host-skills-into-registry.md) · **模型** sonnet · **独测** ✅ · **状态** 待做

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
