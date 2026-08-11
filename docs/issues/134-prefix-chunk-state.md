# 134 前缀块状态：开局结果落 store

**里程碑** M15 · **依赖** — · **模型** **opus** · **独测** ✅ · **状态** 完成（2026-08-11）

## 目标

给「开局工具的结果」在 core 一个家：一次写入、随会话落盘与恢复、组料可读、
undo 退不掉。core **不知道内容来自工具**——对它而言这只是「会话创建期定下的
一列带 label 的文本块」（红线 12 的精神：core 不认识「时机」「skill」任何一个词）。

## 现状

system 料今天 = 基础 system + `skill_injection` 现算的 `late_system`
（`provider_call.rs:158-190`）。没有「创建期算出、会话级不变」的前缀槽。

先例：M10 的宿主声明状态就是「创建期写入、恢复原样回来」的同类
（`agent-core/tests/it/host_tools_indep_restore.rs` / `host_skills_indep_restore.rs`），
undo/恢复语义对齐它们。

## 做什么

1. `Slot` 新变体（建议 `PrefixChunks`），值 = `Vec<SystemChunk>`（label + text 都是
   `Arc<str>`，可序列化——红线 3；`Vec` 保序——红线 11）。
2. `Session::set_prefix_chunks(chunks)`：创建期一次性写入，走 command 层（红线 2），
   entry label `"prefix_init"`。重复写入按既有「逻辑错误」先例处理。
3. `Session::prefix_chunks()`：组料方读。
4. undo 语义**逐字对齐 `declare_host_skills` 先例**（`command/host_skills.rs` 的
   测试 `a_declaration_is_journaled_and_undo_takes_it_back`）：写入是一条
   journaled entry，**entry 级 undo 能退掉、redo 能回来**；用户面的 turn 级
   undo 永远到不了它（它在第一轮之前，不属于任何 turn）。别发明「不可 undo
   的状态」——单一线性日志（决策 4）没有这个概念。
5. 快照/恢复 roundtrip 逐字节；恢复路径**不重算**（值就是状态，这是 135 那条
   「不重跑」验收的 core 侧前提）。

## 验收

- `set_prefix_chunks` 两块 → `prefix_chunks()` 逐字节相同，顺序 = 写入顺序。
- 快照 → 恢复 → `prefix_chunks()` 逐字节相同（含顺序）。
- 写入产生一条 entry（label `"prefix_init"`）；entry 级 undo 退掉后为空、
  redo 恢复（host_skills 先例同款断言）；空 `Vec` 写入不产生 entry（幽灵条目）。
- serde roundtrip 测试；`check-invariants` 过（值类型无 `HashMap`/`HashSet`）。

## 注意

- **红线 2/3/4/11 全在场**。opus 的依据：恢复时读错值**不报错**，只在「重启后第一轮
  prompt 悄悄变了」时以缓存归零或行为漂移浮出来——最贵的静默类。
- 独测 agent 只看本文件验收 + `pub` 签名 + 红线 2/3/4/11 条目。
- 别在本条里接驱动或组料（135 的事）；本条交付的是状态与它的四条投影
  （undo/redo/落盘/恢复）一致。

## 实做记录（2026-08-11）

- 落点：`value/prefix_chunks.rs`（161 行）、`command/prefix.rs`（154 行）；`Slot::ALL` 18→19。
- 登记点共 8 处，其中 **`command/meta.rs` 的 `KNOWN_LABELS` 加 `"prefix_init"` 是 grep
  `Slot::HostSkills` 找不到的**——顺 label 先例找到；漏了它 = 新会话文件 `recover` 时硬失败。
- 拍板一条：`from_value` 形状不对**整份读空**而非逐项跳过（跳中间一块会拼出「少一段但每轮
  照发」的 system 前缀，理由在函数文档）。独测 `prefix_chunks_indep.rs` 六条全绿。
