# 101 第 2 档：清工具返回的 command

**里程碑** M12 · **依赖** [100](100-projection-into-ingredients.md) · **模型** sonnet · **独立测试 agent** 是 · **状态** 完成

## 目标

把「清掉某几条工具返回」做成一个正规的状态变更：走 command 层、进 undo log、能撤。

只做**写路径**。什么时候清、清谁，是 [102](102-clear-tool-results-policy.md) 的事。

## 做什么

一个 command，输入是一批 `ToolCallId`，效果是把它们并进 `SendPlan` 的已清列表。

**内容一个字节都不删**——完整记录在 `SessionStore` 里原样躺着（095 的分界），
这里改的只是「发不发」。所以 `prev` 只有已清列表的旧值。

## 定死的接口（2026-08-10 主会话定，实现与测试都照这个来）

```rust
impl Session {
    /// 第 2 档：把这批工具调用的**结果**标记为「不再发送」。
    ///
    /// **内容一个字节不删**——完整记录在 `SessionStore` 里原样躺着（095 §2），
    /// 改的只是 `SendPlan` 的已清列表。
    ///
    /// - **只接受在这个 agent 的历史里真实存在的 `ToolResult` id**。不存在的
    ///   忽略并计入 `unknown`——静默接受会让 102 的 bug 藏在一个永远不生效的
    ///   id 里，而那不报错、不影响功能，只是压缩没压到。
    /// - **幂等**：已在列表里的不重复加入，计入 `already_cleared`。
    /// - **`newly_cleared` 为空时不写、不进 undo log**——不留空 entry。
    pub fn clear_tool_results(
        &mut self,
        agent: &AgentId,
        ids: impl IntoIterator<Item = ToolCallId>,
    ) -> ClearOutcome;
}

/// 一次 `clear_tool_results` 的记账。三个桶互不相交，并集 = 入参去重后的集合。
#[derive(Clone, PartialEq, Debug)]
pub struct ClearOutcome {
    /// 本次真正新加进已清列表的。
    pub newly_cleared: Vec<ToolCallId>,
    /// 已经在列表里，幂等跳过的。
    pub already_cleared: Vec<ToolCallId>,
    /// 这个 agent 的历史里找不到对应 `ToolResult`，已忽略的。
    pub unknown: Vec<ToolCallId>,
}
```

不加 `#[must_use]`：不取走返回值不会造成状态自相矛盾（清除已经真实发生），
顶多是调用方没看见记账。照 `History::take_drop_events` 的先例。

底层复用 100 的 `replace_send_plan`（不含策略的整体替换）；本条负责它之上、
102 之下的那一层：**校验 + 幂等 + 记账**。

## 验收

- 清 50 条工具返回后 `/undo` 一次，**50 条全回来**（下一轮 `encode` 里重新出现）
- 该 entry 的 `prev` **序列化后 < 1 KB**——它装的是一串 id，不是内容
- `redo` 一次，50 条重新消失
- 清完之后**这个 agent 的消息条数不变**（`messages_of(agent).len()`），完整记录一条没少
  ——**这条原文写的是「`History.entries()` 长度不变」，写错了**：`History` 是 undo
  日志，一次进日志的写入本来就该多一条 entry。两个 agent 各自独立读成了「消息条数」，
  那才是本意（095 §2 的「存的」那一侧）
- 重复清同一个 `ToolCallId`：幂等，已清列表不出现重复项，`prev` 也不因此变化
- 清一个不存在的 `ToolCallId`：不 panic，不静默成功（要么拒绝、要么记为无操作，
  二选一并写明）

## 注意

- **红线 2**（禁 `store.set()`）——这是本 issue 的主线，业务代码一律走 command 层。
  漏了它 undo 就是空的，而且不报错
- **红线 5**——已清列表会长，但它装的是 id，不是大值；真长到需要 `Arc` 时再说，
  别提前包
- **红线 11**——已清列表进 prompt（它决定发什么），容器禁 `HashMap`/`HashSet`
- cap 的账：这条 entry 小，`history/cap.rs` 的 100 条上限吃得下——这正是 095
  那个形状决策的兑现点，验收第二条就是它的度量

## 实做记录（实现 agent + 独立测试 agent 并行，2026-08-10）

与 [104](104-boundary-command.md) 同时开工，两支互不碰对方的文件；共享的
`command/mod.rs` / `tests/it/main.rs` 各追加一行，零冲突（018/019/020 的先例）。

### 落地的文件

| 文件 | 行数 | 职责 |
|---|---|---|
| `agent-core/src/command/clear_tool_results.rs`（新建） | 300 | `clear_tool_results` 写路径 + `ClearOutcome` + 内联单测（实现体只占前 106 行） |
| `agent-core/src/command/mod.rs`、`lib.rs` | 各 +1~3 | 挂载与根导出 |
| `agent-core/tests/it/clear_tool_results_*.rs`（新建 7 个） | 46/149/71/44/51/69/51 | 独测：fixture、undo/redo、幂等、unknown、三桶、空操作、插入顺序 |

**⚠️ `clear_tool_results.rs` 正好 300 行，顶在天花板上。** 报告里说是「几次精简后
压进预算」——[CLAUDE.md](../../CLAUDE.md) 的规矩是「上限是天花板不是目标，
按职责拆不按行数凑」。下次动这个文件的人第一件事应该是把 `ClearOutcome` 拆出去，
而不是继续删注释。

### 设计判断（实现 agent 裁决，主会话复核后收）

1. **unknown id 记账不拒批**：一个坏 id 不该拖垮同批另外 49 个有效的；
   而静默全盘接受会把 102 将来的 bug 藏进一个永远不生效的 id 里。
2. **先判存在、再判已清**：一个 id 得先是真的，才谈得上「已经清过」。
3. **同一次调用内的重复 id 按首次出现归类一次**（接口没写，实现自己裁的，有测试）。
4. `newly_cleared` 为空时显式不调 `replace_send_plan`。

### 变异检验（主会话做）

**第一次变异是等价变异，值得记下来**：把「`newly_cleared` 为空就不写」这个分支拿掉
（改成无条件写），13 个测试**全绿**。查下去发现不是测试有洞——
`History::append` 在 `changes.is_empty()` 时直接返回 `None`
（`agent-store/src/history/log.rs:105`），而值没变时 `record_set` 根本不产生 change。
**所以判断 4 那个显式分支是冗余优化，不是正确性守卫**，实现 agent 报告里说的
「少一次 clone/commit」才是它真正的价值。

第二次变异（取消 unknown 校验，一律当作可清）：**5 个测试红**
——`clear_tool_results_unknown` 两个、`no_op_no_entry` 两个、`outcome_buckets` 一个。
守卫是实的。

### 命令输出

```
$ cargo test --workspace
1640 passed; 0 failed

$ cargo clippy -p agent-core --all-targets -- -D warnings
干净

$ bash scripts/check-invariants.sh --all
exit 0；17 条行数提示全是存量文件

$ 实测 prev 大小
24–51 字节（要求 < 1 KB）
```
