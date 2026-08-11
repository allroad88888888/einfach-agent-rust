# 138 `srv:skill/index`：索引文本（实现，不装配）

**里程碑** M15 · **依赖** — · **模型** sonnet · **独测** ✅ · **状态** 完成（2026-08-11）

## 目标

实现将来以 `SessionStart` 时机注册的索引工具：无入参 → 一段文本，首行一句
「以下 skills 可用 `srv:skill/read` 按 id 取全文」，之后每个 skill 一行
`<id> — <description>`，按 id 字典序。空 registry → 空文本（135 的规则会让
空文本不产前缀块）。**只实现不装配**（139 的事）。

## 现状

常驻索引今天由 `with_skills` 直接产 `INDEX_LABEL` 的 `SystemChunk`
（`skill/mod.rs`）——本条实现的是它的接替者，格式可以直接参照。
registry 的键已经是 `BTreeMap`（红线 11 的既有保证），迭代序即字典序。

## 做什么

索引函数拿 `&SkillRegistry` → `String`。放 `skill/read.rs` 旁或独立小文件
（按行数与单一职责判）。

## 验收

- 同一 registry 两次调用**逐字节相同**。
- 覆盖 0 / 1 / N 个 skill；N 时行序 = id 字典序（与目录装载顺序无关——
  「后来居上」合并之后键序是唯一序）。
- 输出**不含任何正文字节**（只 id + description）——索引泄漏正文等于回到
  「全量常驻」，钱和缓存两头都输。
- description 里带换行的 skill：折成单行（索引一行一个 skill 是格式契约）。

## 注意

- **红线 11**：这段文本每个会话进 system 前缀，逐字节确定是硬要求。
  `BTreeMap` 迭代已保证，别用 `HashMap` 中转、别拼时间戳。
- 树形（142）会在这上面加 hidden 过滤——本条先列全量，别预留半实现的参数。

## 实做记录（2026-08-11）

- 落点：`skill/index.rs`（162 行）：`SkillRegistry::index_text()`，首行引导 + 每行
  `<id> — <description>`（字典序 = BTreeMap 迭代序），换行折空格，空 registry 空串。
- 独测 `skill_index_indep.rs` 六条全绿（目录遍历序 ≠ 字典序的 fixture、正文哨兵断言不泄漏）。
