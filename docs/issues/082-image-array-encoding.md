# 082 wire 的数组编码：有图才用数组

> ⚠️ **已废弃（superseded by s5）**：本文描述的 images 管线（`ContentBlock::Image` /
> `POST /files` / `upload_base_url` / `ImagesDropped` / 前端选图 / vision 子 agent 委托）
> 已被 s5 重构整体移除，现以 `POST /uploads` 上传端点 + `srv:vision/inspect` 工具取代。
> 正文仅作历史决策档案保留，不再反映当前实现。

**里程碑** M11 · **依赖** [079](079-image-content-block.md) · **模型** sonnet · **独测** ✅（红线 11：前缀字节） · **状态** 完成

**先读 [docs/IMAGES.md](../IMAGES.md) §五**（为什么是「有图才用数组」而不是恒定数组——
那是拍过的，**别顺手改成恒定数组**）。

纯函数层，**零 IO，全部对着构造出来的 `Message` 单测**，本 issue 不该有任何真机调用。
本 issue 只做**机制**；哪家吃得下图是 [083](083-image-provider-fallback.md) 的活。

## 范围

`crates/agent-providers/src/wire/messages.rs` 的 `push_message`（56–106 行）：

1. **消息里没有图片块 → 逐字节维持现状**（`content` 是字符串）。
   现有那批 golden 测试**一个都不该改**。改到了就是走偏了。

2. **有图片块 → `content` 是数组**：
   ```json
   [{"type":"text","text":"..."},
    {"type":"image_url","image_url":{"url":"<reference>"}}]
   ```
   `reference` **原样**放进 `url`，adapter 不解析它（IMAGES.md §七）。
   块顺序照 `Message.blocks` 的顺序，不重排。

3. **「这家吃不吃图」由调用方传进来**，不许在 `messages.rs` 里 `match provider`
   （红线 12——这个文件三家共用）。建议形状：`history()` 多收一个参数、
   返回值多带一个**被降级的图片张数**。形状可以换，**判断必须落在各家 `encode` 里**。

   本 issue 只把这个口子开出来并给出默认行为；**三家各传什么值是 083 的活**。

4. 079 在这里留的那个 `ContentBlock::Image { .. } => {}` 占位分支，本 issue 换成真编码。

## 验收（可判定）

1. **无图 = 逐字节不变**：现有 golden 断言（如
   `json!({"role":"user","content":"北京天气"})`）**原样通过，一个字符都不改**。
2. **有图 = 形状对**：吃得下图时产出的 `content` 是数组，`image_url.url` 与
   `reference` **逐字符相同**；文本块在前、图片块在后。
3. **两张图**：两个 `image_url` 都在，顺序与 `blocks` 一致。
4. **确定性**（红线 11）：同一条带图消息编两次，产出的 JSON **逐字节相同**。
5. **纯文本消息永远不变成数组**：哪怕同一条历史里别的消息带图。

## 突变验证（必做）

- 把「无图也走数组」打开 → 第 1 条必须变红。
- 把图片块的顺序改成插到文本前面 → 第 2 条必须变红。

两条都真跑红一次，报文抄进实做记录。

## 注意

- **别碰** `wire::names` / `prefix` / `canonical` 的既有形状（024/039 有测试钉着）。
- 红线 9：`messages.rs` 现在 174 行。加完**如果顶破 300 就拆**，按职责切
  （「一条消息 → wire 消息」和「一个块 → wire 块」是两件事），
  不留「下次再拆」，也不许塞进 `utils.rs`。没顶破就别为拆而拆。
- 收工验证前台跑完，含 `--features ts`。

## 实做记录

- `history_with_image_support(messages, supports_images)` 保持共享 wire 层不认识
  provider；无图仍编码为字符串，有图且调用方声明支持时才编码为 OpenAI 兼容数组。
  同一个历史中的纯文本消息不随之改变形状。
- 独测（实现恢复后）：`cargo test -p agent-providers --lib wire::image_encoding_tests`，
  2 passed；覆盖无图字节不变、两图顺序、引用原样传递和重复编码字节确定。
- 突变一：把两个 `has_image && supports_images` 改成 `supports_images` 后运行
  `cargo test -p agent-providers --lib wire::image_encoding_tests::text_only_wire_content_remains_exactly_string_even_when_images_are_supported -- --exact`，目标断言首先变红：

  ```text
  thread 'wire::image_encoding_tests::text_only_wire_content_remains_exactly_string_even_when_images_are_supported' (42564310) panicked at crates/agent-providers/src/wire/image_encoding_tests.rs:33:5:
  assertion `left == right` failed: 无图消息必须保持既有字符串 content，不能改成数组
    left: [Object {"content": Array [Object {"text": String("北京天气"), "type": String("text")}], "role": String("user")}]
   right: [Object {"content": String("北京天气"), "role": String("user")}]
  ```

- 突变二：把图片 `content.push` 改成 `content.insert(0, ...)` 后运行
  `cargo test -p agent-providers --lib wire::image_encoding_tests::image_wire_content_is_text_then_all_images_in_block_order -- --exact`，目标断言首先变红：

  ```text
  thread 'wire::image_encoding_tests::image_wire_content_is_text_then_all_images_in_block_order' (42566935) panicked at crates/agent-providers/src/wire/image_encoding_tests.rs:68:5:
  assertion `left == right` failed: 图片引用必须原样写入，且文本块始终在图片块之前
    left: Array [Object {"image_url": Object {"url": String("ms://second")}, "type": String("image_url")}, Object {"image_url": Object {"url": String("ms://first")}, "type": String("image_url")}, Object {"text": String("请看附件"), "type": String("text")}]
   right: Array [Object {"text": String("请看附件"), "type": String("text")}, Object {"image_url": Object {"url": String("ms://first")}, "type": String("image_url")}, Object {"image_url": Object {"url": String("ms://second")}, "type": String("image_url")}]
  ```
