# 079 `ContentBlock::Image` 变体

> ⚠️ **已废弃（superseded by s5）**：本文描述的 images 管线（`ContentBlock::Image` /
> `POST /files` / `upload_base_url` / `ImagesDropped` / 前端选图 / vision 子 agent 委托）
> 已被 s5 重构整体移除，现以 `POST /uploads` 上传端点 + `srv:vision/inspect` 工具取代。
> 正文仅作历史决策档案保留，不再反映当前实现。

**里程碑** M11 · **依赖** — · **模型** **haiku** · **独测** ✅（红线 3/5，但独测能把它变成会红的断言）· **状态** 完成

**先读 [docs/IMAGES.md](../IMAGES.md) §七**（为什么 `reference` 对 core 不透明）。
本 issue **只加一个变体**，不碰 wire、不碰事件、不碰 IO、不碰前端。做完之后图片还发不出去，
这是对的。

**无依赖，可以立刻开工，跟 [080](080-adjustment-images-dropped.md)、
[084](084-transport-files-upload.md) 并行。**

## 范围

`crates/agent-core/src/value/message.rs` 的 `ContentBlock` 加一个变体：

```rust
/// 一张图。**`reference` 对 core 完全不透明**——只存、只原样传给 adapter，
/// 不 match、不解析、不按前缀分支。跟 `ToolCallId` 同一类（provider 铸的
/// 字符串，core 原样存原样传回）。红线 12 禁的是判断，不是数据；
/// 理由与反例见 docs/IMAGES.md §七。
Image {
    reference: Arc<str>,
    /// MIME，如 `image/png`。给 adapter 编码用，也给降级占位文本用。
    mime: Arc<str>,
    /// 原始文件名，给人看。没有就是 None。
    name: Option<Arc<str>>,
},
```

三个字段一律 `Arc<str>`（红线 5：`store.get()` 每次读整条历史都要 clone）。

## 四处落点，已经替你查过了

**只有一处会编译报错**，另外三处是 `_ => ...` 兜底，**编译器不会提醒你**——
所以逐处列在这里，别再自己找一遍：

| 位置 | 现状 | 本 issue 怎么处理 |
|---|---|---|
| `agent-providers/src/wire/messages.rs:62` | **真穷举 match**，四个分支无兜底 | **会编译报错**。本 issue 只加 `ContentBlock::Image { .. } => {}` 让它编译过，**并在旁边留一行注释指向 [082](082-image-array-encoding.md)**。真正的编码是 082 的活，这里不要提前写 |
| `agent-core/src/observe.rs:129` | `_ => None`（`first_user_text` 取任务标题） | **保持不动**。已确认正确：任务标题只该是文本，图片不贡献标题 |
| `agent-core/src/command/transitions/provider_done.rs:42` | `_ => None`（只捞 `ToolUse`） | **保持不动**。已确认正确：图片不是工具调用 |
| `agent-runtime/src/child_outcome.rs:55` | `_ => None`（收子 agent 的文本产出） | **保持不动**。已确认正确：图片不是文本，且子 agent 目前不产图 |

后三处保持不动**是结论不是省事**——把它判过的这件事写进实做记录，
下一个人才不用重查一遍。

## 验收（可判定）

1. `cargo test` 全绿、`cargo clippy --all-targets` 零 error。
2. **序列化往返**（红线 3）：`message.rs` 已有的 `message_roundtrip` 测试里加上
   `ContentBlock::Image`（三个字段都填非空值，`name` 再补一个 `None` 的用例），
   `to_string` → `from_str` → **相等**。
3. **`name: None` 与 `name: Some("")` 往返后仍然分得开**（别被 serde 的默认值吃掉）。

## 突变验证（必做）

把 `reference` 字段的 `Deserialize` 换成永远返回空串 → 第 2 条必须变红。
改完恢复原状，把红的那条报文抄进实做记录。**没红过的护栏不算护栏。**

## 注意

- 红线 12：core 里**不许**出现 `reference.starts_with("ms://")` 这类分支。
- `ContentBlock` 目前**没有**挂 ts-rs 导出（`agent-server/src/ts_protocol/export.rs`
  里搜不到）。**本 issue 不要给它加导出**——真需要是 086 的事。
- 收工验证前台跑完，含 `cargo test --features ts`（WORKFLOW §四 -1）。

---

## 实做记录（完成 · 2026-08-04）

- `ContentBlock` 新增了只保存数据的 `Image`；三个值字段均为 `Arc<str>`，没有对
  `reference` 做前缀、内容或 provider 判断。
- `message_roundtrip` 同时覆盖三个非空字段和 `name: None`；
  `image_name_none_and_empty_roundtrip_differently` 用真实 JSON 往返并断言 `None` 与
  `Some("")` 仍不相等。
- `wire/messages.rs` 是唯一需要穷举补臂的位置，079 的空臂只维持编译通过并明确指向 082。
  `observe.rs` 的任务标题、`provider_done.rs` 的工具调用和 `child_outcome.rs` 的子 agent
  文本三个 `_ => None` 均保持不动：图片分别不应贡献标题、不是工具调用、也不是文本产出。
  没有增加 ts-rs 导出。
- 合并后的全 workspace 编译还揭露了测试辅助函数
  `agent-runtime/tests/spawn_bg_support::any_message_mentions` 的穷举匹配；图片不含可供该
  文本搜索使用的内容，因此补为 `ContentBlock::Image { .. } => false`。这是 079 新变体的
  编译兼容性落点，不改变其余三处已判定的语义。

### 突变验证：先红后恢复

临时把 `Image.reference` 的 `Deserialize` 改为永远产出空串，随后运行
`cargo test -p agent-core message_roundtrip`。目标断言 `message_roundtrip` 在图片引用的
`assert_eq!` 处变红；恢复正常反序列化后，针对 `message` 的回归测试包含
`message_roundtrip` 与 `image_name_none_and_empty_roundtrip_differently` 均通过。红报文原样如下：

```
   Compiling agent-core v0.1.0 (/Volumes/work/self/einfach-agent-rust/crates/agent-core)
    Finished `test` profile [unoptimized + debuginfo] target(s) in 7.40s
     Running unittests src/lib.rs (/private/tmp/einfach-agent-rust-079-mutation/debug/deps/agent_core-3168757eda04b7ac)

running 1 test
test value::message::tests::message_roundtrip ... FAILED

failures:

---- value::message::tests::message_roundtrip stdout ----

thread 'value::message::tests::message_roundtrip' (42332895) panicked at crates/agent-core/src/value/message.rs:124:9:
assertion `left == right` failed
  left: Message { id: MessageId(1), role: Assistant, blocks: [Text("hello"), Thinking("thinking..."), ToolUse { id: ToolCallId("call_1"), name: "fs/read", input: Object {"path": String("/tmp/a")} }, ToolResult { id: ToolCallId("call_1"), content: "file contents", is_error: false }, Image { reference: "", mime: "image/png", name: Some("original.png") }, Image { reference: "", mime: "image/jpeg", name: None }] }
 right: Message { id: MessageId(1), role: Assistant, blocks: [Text("hello"), Thinking("thinking..."), ToolUse { id: ToolCallId("call_1"), name: "fs/read", input: Object {"path": String("/tmp/a")} }, ToolResult { id: ToolCallId("call_1"), content: "file contents", is_error: false }, Image { reference: "opaque-reference", mime: "image/png", name: Some("original.png") }, Image { reference: "another-opaque-reference", mime: "image/jpeg", name: None }] }
note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace


failures:
    value::message::tests::message_roundtrip

test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 90 filtered out; finished in 0.00s

error: test failed, to rerun pass `-p agent-core --lib`
```

### 注意

- `message_roundtrip` 的末尾 `assert_eq!` 是序列化护栏；把反序列化弄坏时必须让它先红，不能
  用解析 JSON 失败之类的构造代替它。
- 全 workspace 的 `cargo test`、`cargo test --features ts`、`cargo clippy --all-targets` 与
  `scripts/check-invariants.sh --all` 由主会话在合并收工时前台代收；这是为避免与并行 issue
  争用 Cargo target lock。079 没有触及协议导出类型。
