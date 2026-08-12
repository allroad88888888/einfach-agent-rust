# 155 `ToolTable::with_host_prefix`：声明合成常量文本 timed 工具

**里程碑** M17 · **依赖** — · **模型** sonnet · **独测** ✅ · **状态** 未开始

## 目标

决策 31 的机制核心：把宿主声明的 `(name, text)` 对合成为
**「执行体 = 返回常量文本」的 `SessionStart` timed 工具**，追加进这个会话的表。
这一合成让三处既有机制**零改动**认识它：

- `run_session_start` 照常跑它 → 文本落 `init:<name>` 前缀块（135 的契约）；
- `check_prefix_allowed`（spawn 的 `inherit_prefix` 校验）读的就是 timed 区
  spec 名 → 子 agent 能点名要/不要这个块；
- 恢复路重建表时重新合成 → 恢复后 spawn 的校验行为与重启前一致
  （run 不会被再调，值从 134 的状态回放——既有语义，一行不动）。

## 做什么

1. `agent-runtime`：`ToolTable::with_host_prefix(pairs: &[(Arc<str>, Arc<str>)]) -> ToolTable`
   ——每对合成一条 timed 条目（spec 的 description/schema 用兜底值，反正
   timed 工具永不进模型面），`CallTiming::SessionStart`，执行体
   `move |_, _, _| Ok(text.clone())`。
2. **条目间按 name 排序后注册**（154 进店前已排一次，这里再排是防调用方
   没走店直接喂——两道闸各自成立）。空切片 = 返回的表**逐字节等于** self。
3. 表尾原则：调用点约定 `with_host_prefix` 排在装配链**最后**（在
   `with_host_tools` 之后）——内置 timed（skills 索引）先注册，声明块的
   前缀块因此永远排在内置块之后，所有会话共有的那段字节不动。

## 验收

- 两对声明 → `run_session_start` 后 `prefix_chunks()` 恰两块，label
  `init:<name>`、text 原样、序 = name 序；乱序喂入结果不变（排序断言）。
- `check_prefix_allowed(vec!["<声明名>"], &table)` 通过；未声明的名照旧拒。
- 空切片：表与 self 逐字节相同（specs/declares/timed 三面都断言）。
- 与内置 timed 共存：内置索引块在前、声明块在后（注册序断言）。

## 注意

- 红线 11：排序 + 注册序即前缀块序，别经任何无序容器中转。
- 合成条目**不进** `specs()`/`declares()`（timed 区既有语义，133），断言钉一条。
- 不碰协议/store——纯 runtime 机制，入参就是排好的对子。
