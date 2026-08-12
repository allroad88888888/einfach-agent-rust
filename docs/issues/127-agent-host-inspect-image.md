# 127 `AgentHost.inspectImage`：Rust 侧的识图协议

**里程碑** M14 · **依赖** [125](125-fetch-post-json-async.md) + [126](126-vision-pure-logic.md) · **模型** sonnet · **独测** 真机 · **状态** 待做

## 目标

浏览器宿主暴露一个识图入口：给字节，还文本。

**这是 [119](119-browser-host-capability-decision.md) §四那张分工表里「Rust 那一格」的
全部内容**——不含存储、不含选图、不含链接解析，那些都是页面的事。

## 接口（定死）

```rust
// agent-wasm/src/host.rs
/// 把一张图交给识图服务（Kimi 3），拿回文字描述。
///
/// **字节只在这一次调用里存在**：上传换成 `ms://` 引用之后就不再被引用，
/// 不进消息历史、不进 prompt、不进 journal。
///
/// - `bytes`：图片原始字节，**上限 2 MiB**（浏览器侧的数，不是
///   `MAX_IMAGE_BYTES` 那个 100 MiB——见 119 §五-1）
/// - Promise reject 的 message 里**不含 api key**
#[wasm_bindgen(js_name = inspectImage)]
pub fn inspect_image(&self, bytes: Vec<u8>, mime: String, question: String) -> js_sys::Promise;
```

实现 = `upload_image_async`（113 已有）→ `ms://<id>` →
[126](126-vision-pure-logic.md) 的 `chat_body` → [125](125-fetch-post-json-async.md) 的
`post_json_async` → `parse_content`。**四步都是现成件，本条只是接线。**

## 做什么

1. 接上面四步。
2. **2 MiB 闸**：定义一个 `agent-wasm` 自己的常量，
   **不要复用 `agent_transport::MAX_IMAGE_BYTES`**——那是 Moonshot 的 100 MiB
   传输上限，跟浏览器的存储配额是两回事。两个常量都要在各自的文档注释里
   指向对方，说明为什么不是同一个数。
3. Kimi 的 base_url / key / model 从哪来：`HostConfig`（114d）今天只装一家
   provider 的配置。识图**写死 Kimi 3**（`vision_inspect.rs:1` 那条），
   所以当主对话用的是 DeepSeek 时，Kimi 的 key 从哪来是一个必须回答的问题。
   **由实现者定并在实做记录里写明**——但有一条硬约束：
   **不许有任何默认 key，也不许把 key 打印/序列化出去**（111 契约第 4 条，
   `host.rs:20-23` 与 `config.rs` 已经为主 provider 兑现过一次，这里照做）。
4. 没配 Kimi key → Promise reject，措辞对齐 `vision_inspect.rs` 的
   `not_configured`，**不 panic**。

## 验收

- `bash scripts/build-wasm.sh` 过。
- **真机主证据**：页面上传一张有可辨认内容的图（例如写着四位数字的图片），
  调 `inspectImage`，拿到的文本**答对那个数字**。
  这跟 `probes/PROVIDERS.md §八` 当年验 Kimi 识图是同一条判据。
- **真机**：超过 2 MiB 的字节 → reject，措辞说得出是大小问题；
  **且没有发出任何网络请求**（DevTools Network 面板确认）。
- **真机**：没配 Kimi key → reject 且措辞是 `not_configured` 那一类，页面不崩。
- **key 不泄漏**：上面三条 reject 的 message 里都不含 key。
  这条同时靠 [125](125-fetch-post-json-async.md) 的 `redact`。

## 注意

- **这条不接工具、不接模型。** 它是一个页面可以直接调的方法，验收也是页面直接调。
  把它接成一条 `web:source/vision` 工具是 [130](130-browser-vision-end-to-end.md)。
  两条分开的理由：识图协议对不对、和模型会不会用它，是两个独立的失败面，
  混在一起调不动。
- `host.rs` 今天 195 行，模块文档开头写着「**五件事**——建会话、发一句话、拿流式
  增量、取消、切会话」。M14 会给它加好几件（[121](121-js-tool-callback.md) 的
  `onToolCall`、[122](122-page-declared-tools.md) 的声明入口、
  [128](128-idb-images-store.md) 的 `deleteSession`、本条的 `inspectImage`）。
  **「五件事」这个数字和这个列表要跟着改**，而且 `host.rs` 必然顶破 300 行——
  拆分是这几条 issue 里**第一个碰到上限的人**的责任，不是最后一个。
