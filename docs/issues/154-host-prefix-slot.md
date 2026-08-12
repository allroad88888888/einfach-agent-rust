# 154 `Slot::HostPrefix`：宿主声明的开局块进 store

**里程碑** M17 · **依赖** — · **模型** sonnet · **独测** ✅ · **状态** 未开始

## 目标

决策 31 的状态位：宿主经 `capabilities.prefix` 声明的开局块（`(name, text)` 对）
是**会话状态**，跟 073 的 `Slot::HostTools` 完全同构——建会话时 journaled 写进
store，恢复时从日志回放自动回来，宿主不用（也不许）恢复时再说一遍。

## 现状

- `Slot::ALL` 现在 **20** 个；`value/host_tools.rs` + `command/host_tools.rs`
  （`declare_host_tools` / `host_tools()`）是要照抄的形状。
- `KNOWN_LABELS` 里 073 用的 label 是既有先例，翻一眼再定本条的。

## 做什么

1. `agent-core`：`Slot::HostPrefix`（`ALL` 20→21），default 空。
2. `value/host_prefix.rs`：`Vec<(Arc<str>, Arc<str>)>` ↔ `Value` codec。
   **进店前按 name 排序**（红线 11：客户端数组顺序不可信，HOST-CAPABILITIES §六
   规则 2 的既有拍板），all-or-empty 反序列化（照 `prefix_chunks.rs` 先例）。
3. `command/host_prefix.rs`：`declare_host_prefix(Vec<(Arc<str>, Arc<str>)>)` +
   只读 `host_prefix()`，journaled 一条 entry，label 进 `KNOWN_LABELS`。
4. 空声明写入 = 无痕（值等于默认值时 `record_set` 不产生 `Change`，
   照 134 的既有语义，断言钉住）。

## 验收

- 写 → 读 roundtrip 保序（排序后的序）；序列化逐字节确定（两次编码相等）。
- 老快照（没有这个 slot）反序列化不炸，读出默认空。
- 空声明不多一条幽灵 entry。
- undo 一条 `declare_host_prefix` entry → 读口回到空（073 同款断言）。

## 注意

- 红线 3（可序列化）/ 红线 4（落盘用 `AtomKey`）/ 红线 11（禁无序容器）——
  照抄 073 的形状就不会踩，**别自创新编码**。
- 只做状态位，不碰装配和协议——那是 155/156 的事。
