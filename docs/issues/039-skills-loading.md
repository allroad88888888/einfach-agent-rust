# 039 skills 装载全链

**里程碑** M5 · **依赖** 038 · **模型** opus · **独立测试 agent** ✅ · **状态** 待办

## 目标

M5 验收整句：放一个 skill 进目录 → 模型自己发现并激活 → 用上它带的工具 →
`/undo` 连激活一起退掉 → DeepSeek 上不炸前缀。

设计地基全部已有（TOOLS.md §Skills、STATE-MODEL 槽位表、`skills_active` 槽位、
`SystemChunk`/`late_tools` 料位、`late_system` 待加）——缺的只是把线连起来。

## 做什么（骨架，细节等 038 数据后由主会话钉 API 再派）

1. **skill 格式与 registry**（宿主侧）：`skills/<name>/SKILL.md`，frontmatter
   `name`/`description`（进索引的那行）+ 正文（激活时注入）+ 可选 `tools` 声明
   （随激活进 `late_tools`，`source: Skill(id)`）。来源目录：内置 + 项目
   `./skills/`，合并冲突与工具同一套规则（TOOLS.md 明令，别另造）
2. **常驻索引**：`SystemChunk{label:"skill-index"}`，每 skill 一行，排序确定
   （红线 11）；索引变更（装了新 skill）= 前缀变更，如实过第 1 层
3. **激活工具**：`srv:skill/activate`/`deactivate`，runner 截获（spawn 同款），
   写 `skills_active`（**经 command 层，journaled——undo 退激活是白拿的**）；
   `Reversibility::Reversible`
4. **注入**（038 实测已定策略，不再猜）：`Ingredients` 加
   `late_system: &[SystemChunk]`（宁可分不可合）。**每家分策**——
   Kimi/GLM 消息级追加（~100% 保前缀，免费）；**DeepSeek 改 system 段尾部**
   （插新 system 消息会 120x 归零，改现有段尾保 ~91%）。这条差异是 adapter
   的活（红线 12：core 只给 late_system，怎么放各家 encode 自己判）。中途
   激活在 DeepSeek 上仍有成本（改 system 尾 = 该段之后失配），报
   `Adjustment::LateSystemReshapedPrefix{est_cost_multiple}` 或复用现有变体
5. **恢复**：`skills_active` 在快照里，内容从 registry 现取（store 只存激活，
   TOOLS.md 原文）；registry 内容漂移的语义写文档
6. CLI `/skills` 列表；web 显示激活集

## 注意

红线 11：索引与注入内容逐字节确定。红线 2/4：激活必须走 command。
038 已实测：三家都收都听，注入策略分家（Kimi/GLM 消息级、DeepSeek 改 system
尾部）。可行性确认，代价差异进各家 encode。DeepSeek 的「改现有 system 段尾部」
需要 `late_system` 能拼进那一段而非独立消息——adapter placement 的实现要点。
