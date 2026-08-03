# 039 skills 装载全链

**里程碑** M5 · **依赖** 038 · **模型** opus · **独立测试 agent** ✅ · **状态** 完成

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

## 实做记录（M5 落地）

**公开面**（钉死签名 + 一处按既有 pattern 归位的偏离）：
- `agent-core`：`SkillId(Arc<str>)`；`Slot::SkillsActive`（值 `AgentValue::Json` 有序
  数组，默认空数组）；`Session::activate_skill(&AgentId, SkillId) -> Result<(), SkillError>`
  / `deactivate_skill(...)`（经 command，label `activate_skill`/`deactivate_skill`，
  进 `known_label` 封闭集）；`SkillError{NotInSession,NotLive,AlreadyActive,NotActive}`；
  `Adjustment::LateSystemReshapedPrefix{est_cost_multiple}`（进 seam + 走 032 TS 重生成）。
  读口按 `read.rs` 既有 `X()`/`X_of(agent)` pattern 落成 `active_skills()`（root）
  + `active_skills_of(agent)`——钉死清单写的是 `active_skills(agent)`，独立验收用的
  是无参 root 形，两者取无参 + `_of`，per-agent 意图保留在 `_of`（见「异议」）。
- `agent-providers`：`Ingredients.late_system: &[SystemChunk]`。
- `agent-runtime`：`SkillRegistry::load(&[PathBuf])` / `skill_index_chunk()`（label
  `skill-index`）/ `listing()`；`ToolTable::with_skills(registry)`（拥有 registry +
  声明两个工具）；`RunnerCtx::available_skills()`；skill 存储/编解码抽进
  `agent-core::value::str_set`（跟 spawn 的 `ToolsAllowed` 共用，红线 11 只一处）。

**每家注入 placement 落点**（`late_system` 一个 skill 一个 `SystemChunk`）：
- DeepSeek `encode.rs`：`system_text_folded(system, late_system)` 把 late 段拼进
  **同一条** system 消息正文尾部；System 段字节变、如实报 `Segment::System` 漂移；
  报 `LateSystemReshapedPrefix{11.0}`（038：改段尾保 ~91%，插新消息 120x 归零）。
- Kimi / GLM `encode.rs`：`late_system_message(late_system)` 追加一条独立
  `{"role":"system","content":…}` 到 history **末尾**（对仅扩展匹配是严格延长 →
  `drift == None`，零代价）；不报 Adjustment。GLM 没有消息级 tools 但**收**消息级
  system（038），这正是「宁可分不可合」保住的差异。
- 空 `late_system` → 三家 encode 逐字节回到 039 之前（向后兼容）。
- 常驻索引进 `Ingredients::system`（宿主放，不是 late）：CLI/宿主调
  `registry.skill_index_chunk()` 塞进 system 前缀，装了新 skill = 索引变 = 前缀变，
  如实过缓存兜底第 1 层。

**undo-激活语义**：激活/停用是 journaled 命令（写 `SkillsActive` 槽），继承所在
root turn 的 `turn_id`——`undo_turn` 一次退整轮连带激活白拿，不需要 skill 专门的
undo 代码。工具侧标 `Reversibility::Reversible`（补偿 = 彼此），走 dispatch 截获、
**不**登记 `mark_irreversible`，所以日志上不留屏障位。redo 重放照样把激活放回来。

**恢复语义**：`SkillsActive` 是 primitive，随快照/日志自动回；skill 正文/工具从
registry **现取**（store 只存激活 id 集，TOOLS.md 原文）。registry 内容漂移
（改了正文、删了 skill）→ `injection` 静默跳过取不到的 id，当它没激活（最不惊扰）。

**已知粗糙度**：`late_system`/`late_tools` = 全部激活 skill（每跳都算），所以一旦
某 skill 常激活，DeepSeek/GLM 每跳都报 `LateSystemReshapedPrefix`/`LateToolsForced‑
IntoPrefix`——跟既有 `late_tools` precedent 一致（非空即报）。skill 稳定不变时该跳
System/Tools 段逐字节相同、不漂、满命中，真实代价 ~1x，兜底第 2 层 predicted vs
实测对账认得出——这条 Adjustment 是「做了这个妥协」的标记（上界），不是每跳账单。

**skill 携带工具的执行**：M5 只做「声明 → 激活时进 `late_tools`（可见）→ 停用/undo
移出」。工具**本体的执行后端**不在范围内（skill 声明的工具名走正常 dispatch →
`ToolExecutor`，没绑后端就 `unknown_tool`）。e2e 只验可见性 + 激活/undo + 注入落点。

### 合并记录 + 真上游验收（主会话，2026-08-03，deepseek-v4-pro）

双侧零分歧：20 独测 + 实现自测全绿，workspace 988/0。**真机 dogfood**：模型看到
常驻索引 → 自己调 srv:skill/activate 激活 commit-cn → 照该 skill 风格给 039 本身
写了提交信息。三点验证：①激活链通（Reversible）；②DeepSeek 报
LateSystemReshapedPrefix{11.0}（038 数据算的 0.91·1+0.09·120）；③**激活后续轮
cached 满命中（12288/12288、1408/1408 一致），不是 120x 归零**——证明用的是
「改 system 段尾」策略而非「插新消息」，DeepSeek 缓存活着。注入分策由实测钉死、
真机兑现。两处偏离（active_skills 无参对齐独测、Slot::SkillsActive 实际新增）收。
undo-激活白拿零专门代码（journaled + turn_id 继承）。
