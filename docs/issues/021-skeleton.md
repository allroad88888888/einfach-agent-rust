# 021 workspace 骨架与最小值类型

**里程碑** M1 · **依赖** 无 · **模型** sonnet · **独立测试 agent** 否 · **状态** 完成

## 目标

`cargo test` 能跑起来，且有**刚好够走通一轮对话**的类型。

## 为什么只定最小集

上一版把 `AgentValue` 一次定到 14 个变体、`Slot` 定到带 `Visibility` 的全集，然后才
发现其中一半在 M1 根本用不上，而用得上的那半有几个形状是错的（见决策 15）。

**没被真实使用验证过的类型定义，跟没写一样，只是它看起来像做完了。**
所以这个 issue 的边界是「022 走通一轮需要什么」，不是「最终会需要什么」。

## 做什么

根 `Cargo.toml` 建 workspace（`resolver = "3"`，`edition = "2024"`），一个 crate：

```
crates/agent-core/
  value/message.rs    Role / ContentBlock / Message
  value/tool.rs       Location / Reversibility / ToolSpec / ToolCallRequest
  value/session.rs    SessionConfig / StopReason / TokenUsage
  ids.rs              ToolCallId / MessageId
```

workspace 依赖固定三个，各自的理由必须写进注释：

| 依赖 | 为什么必须是它 |
|---|---|
| `imbl`（维护中的 `im` fork） | 消息历史 append 要 O(log n) 且结构共享，红线 5 |
| `serde` 开 `derive` + **`rc`** | 红线 3 要求全可序列化，红线 5 要求大值 `Arc` 包住，两条同时成立就必须开 `rc` |
| `serde_json` | `Map` 默认 `BTreeMap`，key 有序——红线 11 依赖这一点，不是巧合 |

`ToolSpec.schema` 用 `Arc<serde_json::Value>`。**别开 `preserve_order` 特性**，
开了 key 顺序就跟插入顺序走，前缀会漂。

## 验收

- `cargo test -p agent-core` 通过（哪怕零测试）
- `cargo clippy --all-targets -- -D warnings` 零告警
- `scripts/check-invariants.sh --all` 通过
- 每个类型都能 `serde_json::to_string` 再 `from_str` 回来，值相等
- **`ToolSpec` 的 `Vec` 序列化两次逐字节相同**（红线 11 的最小实检）

## 注意

- 红线 3：`AgentValue` 的每个变体都要可序列化。M1 只需要少数几个变体，
  但**加变体时这条要一直成立**——加一个不可序列化的进去，崩溃恢复静默丢状态。
- 红线 11：任何会进 prompt 的集合类型禁 `HashMap` / `HashSet`。这个 issue 里
  就是 `ToolSpec` 和它的 schema。
- 红线 9：一个文件一件事，≤300 行。

`Location` 与 `Reversibility` 是**正交**的两个维度（决策 7），不要合并成一个
「工具分类」枚举——前端工具可以不可逆（写剪贴板），服务端工具可以是纯的（读文件）。

## 实做记录

- **`StopReason::Other(&'static str)` 编译不过序列化往返测试，按文档给的退路
  换成 `Arc<str>`。** `&'static str` 要求 `Deserialize<'static>`，只有从编译期
  字符串字面量借用时才成立；`serde_json::from_str` 反序列化一份运行时的
  `String`（比如测试里现造的、或将来 provider 响应体里的原始 `finish_reason`）
  借不出 `'static` 生命周期，往返测试直接编译错误，不是运行时才炸。`Arc<str>`
  保留了「不可变、克隆是指针拷贝」的意图（红线 5），且能正常 serde 往返，已在
  `value/session.rs` 的文档注释里写明取舍，不是偷懒选的近似方案。
- **测试没有堆在 `lib.rs` 里，而是拆回各自模块的 `#[cfg(test)] mod tests`。**
  最初图省事把所有 serde 往返测试和红线 11 实检写进 `lib.rs`（约 200 行，仍在
  300 行硬上限内），但 `lib.rs` 本身只做模块声明和 re-export，塞测试违反
  「一个文件一件事」；也不符合 INVARIANTS.md 红线 9 备注里「源文件里只留最贴身
  的单元测试」的取向——贴身的意思是测试跟它测的类型在同一个文件，不是随便一个
  文件。改成 `ids.rs` / `value/message.rs` / `value/tool.rs` / `value/session.rs`
  各自带自己的测试后，单文件最大是 `value/tool.rs` 207 行（因为红线 11 的
  `Vec<ToolSpec>` 双序更实检和两个方法的穷举断言都堆在这），仍在 300 行以内。
- **edition 2024 没有额外触发新 clippy lint**——`cargo clippy --all-targets -- -D
  warnings` 一次性零告警通过，没有遇到诸如 `if_let_rescope` 之类 edition 2024
  相关的新 lint。`serde` 的 `rc` 特性也没有坑：加上之后 `Arc<str>` / `Arc<Value>`
  字段直接能 derive，没有需要手写 `serialize_with` 的地方。
- 其余按计划：workspace `resolver = "3"`，`imbl` 锁定到当时最新的 7.0.1
  （`features = ["serde"]`），根 `Cargo.toml` 未开 `serde_json/preserve_order`。
  `probes/api` 的独立 `[workspace]` 用 `cargo metadata --no-deps` 验证过确实没被
  拉进主依赖图。
