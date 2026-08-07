# 081 用户输入带图：`Event::UserInput` 与块顺序

> ⚠️ **已废弃（superseded by s5）**：本文描述的 images 管线（`ContentBlock::Image` /
> `POST /files` / `upload_base_url` / `ImagesDropped` / 前端选图 / vision 子 agent 委托）
> 已被 s5 重构整体移除，现以 `POST /uploads` 上传端点 + `srv:vision/inspect` 工具取代。
> 正文仅作历史决策档案保留，不再反映当前实现。

**里程碑** M11 · **依赖** [079](079-image-content-block.md) · **模型** sonnet · **独测** ✅（红线 11：块顺序会进 prompt）· **状态** 完成

**先读 [docs/IMAGES.md](../IMAGES.md)。** 本 issue 让图片**能进历史**，
但还发不出去（编码是 [082](082-image-array-encoding.md)/[083](083-image-provider-fallback.md)，
上传是 [084](084-transport-files-upload.md)/[085](085-http-image-ingress.md)）。

## 范围

1. **`Event::UserInput` 带上图片**。现在是 `{ agent, text: Arc<str> }`
   （构造点 `agent-core/src/observe.rs:161`，转移入口
   `command/transitions/mod.rs:47` → `transitions/user_input.rs:21` 的
   `push_message(Role::User, vec![ContentBlock::Text(text)])`）。

2. **文本仍然是文本块，图片是并列的块**——不许把图塞进文本里拼字符串。

3. **块顺序定死：文本块在前，图片块按宿主给的顺序在后。**
   顺序会进 prompt，两次组装必须逐字节相同（红线 11）。
   **把这条写进 `user_input.rs` 的代码注释**，别让下一个人再猜一遍。

4. **没有图片时逐字节回到本 issue 之前**：`vec![ContentBlock::Text(text)]`
   一模一样，不多一个空 `Vec`、不改顺序。

## 验收（可判定）

1. **不带图 = 老路不变**：纯文本输入产出的 `Message.blocks` 与本 issue 之前
   **完全相同**（要有断言，不是「看起来没变」）。
2. **带图进得去**：一次带两张图的输入 → 历史里那条 `Message` 的 blocks 是
   `[Text, Image, Image]`，两个 `reference` 与传入的一致且顺序一致。
3. **顺序确定**（红线 11）：同样的输入构造两次，`blocks` 序列化出来**逐字节相同**。
4. **undo/redo 复原**：带图的一轮 `/undo` → 历史里没有那条消息；`redo` →
   `reference`/`mime`/`name` **逐字段一字不差地回来**。断言比字段，不是比长度。
5. **落盘恢复**：写 jsonl → 重新 load → 那条图片消息与关掉前逐字段相同。
6. **`Idle` 才收**：`user_input.rs` 现有的「别的状态收到 `UserInput` 都是协议违规」
   那条规则对带图输入同样成立，别为图片开后门。

## 突变验证（必做）

- 把块顺序改成「图片在前」→ 第 3 条必须变红。
- 把 `redo` 路径上 `reference` 的还原改成空串 → 第 4 条必须变红。

两条都要真跑红一次，报文抄进实做记录。**构造得挡得住**：如果改了之后别的断言
先红了，等于这条护栏没验到，换种构造重来。

## 注意

- 红线 11：块顺序、字段顺序都不许依赖 `HashMap`/`HashSet` 的遍历序。
- **turn 边界别动**：`TurnStatus::Idle` 不是终态，`handle_input` 对第一轮不会自己
  开新 turn（STATE-MODEL.md 那条 ⚠）。本 issue 不碰这个语义。
- 收工验证前台跑完，含 `--features ts`。

---

## 实做记录（完成 · 2026-08-04）

- `Event::UserInput` 现在携带 `Vec<UserImage>`；仅图片输入走新分支。无图输入仍字面构造
  `vec![ContentBlock::Text(text)]`，图片输入则固定为文本块在前、后接宿主给定顺序的
  `ContentBlock::Image`。这条 prompt 顺序契约写在 `user_input.rs` 紧邻组装代码处。
- 独立测试覆盖：无图旧块、两图的 `[Text, Image, Image]` 与逐字段引用、同输入的序列化字节
  相同、undo/redo 后 `reference`/`mime`/`name` 逐字段复原、JSONL reopen 恢复，以及非 `Idle`
  的带图输入仍为协议违规。恢复测试通过真实 `RunnerCtx` + JSONL backend 落盘重开，不走 mock。
- 为了让事件测试保持单一职责和普通文件上限，既有 `Event` 单测移动到
  `engine/event_tests.rs`；事件本体与新增测试文件均低于 300 行。

### 突变验证：先红后恢复

临时把图片分支改成先 append 图片、最后 append 文本，运行
`cargo test -p agent-core --test image_user_input image_block_bytes_are_stable_and_text_first`。目标的
顺序字节断言首先变红（实现已恢复）：

```text
thread 'image_block_bytes_are_stable_and_text_first' (42499157) panicked at crates/agent-core/tests/image_user_input.rs:110:5:
assertion `left == right` failed: 文本块必须在图片块之前
```

临时在 `fast_forward` 的 redo 应用路径将重放出的图片 `reference` 改为空串，运行
`cargo test -p agent-core --test image_user_input undo_then_redo_restores_every_image_field`。第 4 条的
字段断言首先变红（实现已恢复）：

```text
thread 'undo_then_redo_restores_every_image_field' (42503586) panicked at crates/agent-core/tests/image_user_input.rs:42:13:
assertion `left == right` failed: reference 必须逐字符保留
  left: ""
 right: "ms://invoice"
note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace
```

### 注意

- 恢复断言必须先将 `Session::messages()` 绑定到局部变量；直接从临时 `Vector` 借用末项会被
  Rust 拒绝（E0716），这不是图片恢复逻辑的失败。
- 全 workspace 的 `cargo test`、`cargo test --features ts`、`cargo clippy --all-targets` 与
  `scripts/check-invariants.sh --all` 由主会话在 079–087 合并后前台代收。
