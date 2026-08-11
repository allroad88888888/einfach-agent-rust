# 137 `srv:skill/read`：正文按需读（实现，不装配）

**里程碑** M15 · **依赖** — · **模型** sonnet · **独测** — · **状态** 完成（2026-08-11）

## 目标

实现一个**普通工具**（时机为空、模型自主调）：入参 `{ skill: "<id>" }` →
返回 registry 里该 skill 的正文全文；未知 id → `is_error` 的 tool_result。
`Pure`。**只实现不装配**——进表在 [139](139-skill-assembly-switch.md)，
分开是为了切换能单独回滚。

这是决策 27 的正文通道：业内主流的「文件读 → tool result」形状，消息尾追加、
前缀零失效，DeepSeek 上不再有段尾税。

## 现状

- 正文已在内存：`SkillRegistry` 装载时 `Skill.body` 就位
  （`agent-runtime/src/skill/mod.rs`）。
- 截获式工具的参照：`skill/tool.rs` 的 activate/deactivate（入参解析、
  可预期拒绝翻成 `is_error`）。
- `reversibility_of` 的 `Pure` 显式名单在 `tool_table.rs`。

## 做什么

1. 新文件 `skill/read.rs`（行数红线，别塞进 `tool.rs`）：spec（name =
   `srv:skill/read`，description 里写明「id 见索引」引导，schema 单字段）+
   执行函数（拿 `&SkillRegistry`，`BTreeMap` 精确查 id）。
2. `reversibility_of` 名单加 `"srv:skill/read"` → `Pure`。
3. 未知 id → `is_error`，文案指向索引，**不列全量 id**（索引已经给过了，
   重复一遍是白花 token）。

## 验收

- 已装载 id → 正文**逐字节**返回（覆盖多行 + 非 ASCII）。
- 未知 id、`"../etc/passwd"` 这类字符串 → 同一条 `is_error` 路，loop 可继续
  （可预期拒绝，对齐 `SkillError` 的哲学：如实回报，不 panic）。
- 执行函数签名只拿 `&SkillRegistry`——**读取路径上没有文件系统**，正文装载期
  已进内存，越界读在结构上不可能（不靠路径清洗）。
- `reversibility_of("srv:skill/read") == Pure`。

## 注意

- **不改 `with_skills`、不动 activate/deactivate**——共存期两条路互不干扰，
  切换是 139 一次做完。
- 红线 11：spec 将来进表，description/schema 的序列化由既有 `serde_json`
  紧凑序列化保证确定，别引入会重排的中间结构。

## 实做记录（2026-08-11）

- 落点：`skill/read.rs`（205 行），截获照 `status_tool` 形状（纯读、当场回写、无 Pending
  无 entry 同步，连 `&Session` 都不要）；`SkillRegistry::body_of` 精确查 id；dispatch 路由
  当时是死路（declares 恒假），139 装配后激活。`reversibility_of` Pure 名单 + 断言。
- 路径穿越在结构上不可能（读的是内存 BTreeMap，不碰文件系统），测试用 `../etc/passwd`
  与未知 id 走同一条 is_error 路钉住。
