# 142 树形：frontmatter `hidden` 不进索引

**里程碑** M15 · **依赖** [138](138-skill-index-tool.md) · **模型** haiku · **独测** — · **状态** 完成（2026-08-11，与 137/138 同一 agent 顺序完成）

## 目标

SKILL.md frontmatter 认一个可选布尔 `hidden`：`true` 的 skill **不进索引**，
但 `srv:skill/read` 照常可读。这就是树形的全部机制——router skill 的正文里
写子 skill 的 id，模型顺着引用递归 read；子 skill 标 `hidden` 免得索引被
几百个叶子撑爆（Codex 给索引设 2% 上下文预算是同一个问题的业内答案）。

**不加 `parent`/`children` 结构字段**——路由是正文的事，不是格式的事。

## 现状

frontmatter 解析在 `skill/yaml.rs`（缩进式 YAML 子集，无外部依赖），
字段传递在 `load.rs` → `Skill`。索引函数是 138 刚交付的。

## 做什么

1. `yaml.rs`：认 `hidden: true` / `hidden: false`；其他值 → `SkillLoadError`
   （既有错误风格，装载期报给部署者——最早可报点）。缺省 = false。
2. `Skill` 加字段，`load.rs` 传。
3. 138 的索引函数滤掉 `hidden`。

## 验收

- `hidden: true` 的 skill：不在索引文本；read 按 id 逐字节取到。
- 无 `hidden` 字段的目录：索引输出与 142 之前**逐字节相同**。
- `hidden: yes` / `hidden: 1` → `SkillLoadError`，进程启动期就报。
- 全部 skill 都 hidden → 索引为空文本（135 规则：不产前缀块）。

## 注意

- 别做「hidden 的 skill 换个二级索引」这类聪明事——发现机制就是父级正文，
  多一条发现路就多一份要维护的真相。

## 实做记录（2026-08-11）

- `yaml.rs` 的 `parse_optional_bool` 只认 true/false，其余 `SkillLoadError::InvalidHidden`
  （装载期报给部署者）；`Skill.hidden` 缺省 false；`index_text` 滤、`body_of`/read 不滤；
  宿主声明的 skill 无 frontmatter 概念，恒 false。全 hidden → 空索引 → 135 规则不产前缀块。
