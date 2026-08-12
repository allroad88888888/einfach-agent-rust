# 144 `Slot::PrefixAllowed`：spawn 快照的前缀授予名单

**里程碑** M15 追加（决策 28） · **依赖** — · **模型** sonnet · **独测** ✅ · **状态** 完成（2026-08-12）

## 目标

core 给「这个子 agent 被授予哪些开局产物」一个家：spawn 时随快照写入、恢复原样
回来、undo spawn 连带退掉。core 不知道「开局产物」是什么（红线 12 的精神：它只是
一份 spawn 时刻定死的名单）。

这是决策 28 的 core 半边；模型面的入参、校验与组料过滤在
[145](145-spawn-inherit-prefix.md)。

## 现状

- **完全同构的先例：`Slot::ToolsAllowed`**（028）——spawn 当时的快照、
  `Null` = 不设限、排序去重落盘（红线 11）、undo 回到 spawn 那一刻用当时的值。
- 编解码先例：`SkillsActive`/`DisabledBuiltins` 共用 `value::str_set`。
- 登记点清单看 134 的实做记录（slot.rs 变体 + `ALL` 数组、slot_default、
  visibility、快照计数断言）——**本条不需要新 entry label**：值在
  `Session::spawn_child` 既有那条命令里同批写入（同一条 entry 多一个 change，
  照 107 apply_summary「两槽一命令一 entry」的先例），`KNOWN_LABELS` 不动。

## 做什么

1. `Slot::PrefixAllowed` 变体（文档注释：是什么、`Null` = 不设限 = 全带、
   为什么随 spawn 快照而不是现查）；`ALL` 数组末尾追加（19→20）。
2. 默认值 `Null`；visibility 照 `ToolsAllowed` 归类。
3. `Session::spawn_child` 签名加 `prefix_allowed: Option<Vec<Arc<str>>>`：
   `Some` 排序去重后落值、`None` 落 `Null`。既有调用点
   （`spawn_tool::intercept`、`compact_spawn`）**全部传 `None`**——本条行为
   零变化。
4. 读口 `Session::prefix_allowed_of(&AgentId) -> Option<Vec<Arc<str>>>`
   （`Null` → `None`），145 的 `system_for` 消费。

## 验收

- spawn 传 `Some(["b","a","a"])` → 读回 `["a","b"]`（排序去重）；传 `None` →
  读回 `None`。
- 快照 → 恢复 → `prefix_allowed_of` 逐字节相同。
- undo 撤掉 spawn → 该 agent 槽位回默认（照 `subagent_indep_undo_spawn` 的手法）。
- 既有调用点全传 `None` 时，全仓测试零变化（唯一例外：写死槽位数的那批断言
  19→20，照 134 的清单逐处更新并写明理由）。

## 注意

- **红线 3/4/11**。独测能把「恢复读错值」变红（roundtrip 断言），所以 sonnet
  够（WORKFLOW §二的两步判据）。
- 别在本条碰 spawn 的 schema/校验/`system_for`——145 的事，分开是为了这条
  能单独验证「加槽位不改行为」。

## 实做记录（2026-08-12）

- 接口零偏离；`spawn_child` 调用点 110 处（28 个文件）全传 `None`，行为零变化；
  槽位计数断言 19→20 逐处更新（despawn 的 evicted 计数也 +1——PrefixAllowed 随
  despawn 逐出，与 ToolsAllowed 的墓碑语义不同，已注明）。
- visibility 归 `Downward`（与 ToolsAllowed 同桶但理由独立成文）。
- 独测 `prefix_allowed_indep` 七条盲写全过：含 `Some([])` ≠ `None` 编码边界、
  带正控的 undo 断言。
- 事故与教训：开发中一次 `cargo fmt` 波及 101 个无关文件，按「worktree 内容 ==
  rustfmt(HEAD)」判据逐个还原；后续 issue 的 agent 约束里已写死「禁对既有文件 fmt」。
