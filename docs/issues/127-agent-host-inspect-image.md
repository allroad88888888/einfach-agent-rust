# 127 `AgentHost.inspectImage`：Rust 侧的识图协议

**里程碑** M14 · **依赖** [125](125-fetch-post-json-async.md) + [126](126-vision-pure-logic.md) · **模型** sonnet · **独测** 真机 · **状态** 完成（真机已验收，见文末）

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

## 实做记录

**接线**：`inspect_image(bytes, mime, question)` 落在新文件
`agent-wasm/src/vision.rs`（160 行）里的 `async fn inspect`，四步照抄接口文档：
`Client::upload_image_async` → `ms://<id>` → `agent_tools::chat_body` →
`Client::post_json_async` → `agent_tools::parse_content`。`host.rs` 里的
`#[wasm_bindgen(js_name = inspectImage)]` 只做 `Rc::clone(&self.inner)` +
`future_to_promise` 包装，不碰 `live`（不需要先 `openSession`）。

**三件必须做对的东西，逐条记决定**：

1. **2 MiB 闸独立成常量**：`vision::MAX_BROWSER_IMAGE_BYTES = 2 * 1024 * 1024`，
   不引用 `agent_transport::MAX_IMAGE_BYTES`。检查顺序是先判断 `vision` 有没有
   配置、再判断大小——**都在建 `Client`、发任何请求之前**，超限的调用路径上
   `agent_transport::Client` 从未被构造。`vision.rs` 模块文档写了正向指向
   （指向 `agent_transport::MAX_IMAGE_BYTES` 说明两者是不同约束层）；反向那句
   （从 `upload.rs` 指回来）**没有加**——本轮改动范围明确排除
   `crates/agent-transport/`，留给下一个碰那个文件的人补一句，不在这条里
   顺手碰不该碰的 crate。

2. **Kimi 的 base_url/key 从哪来**：页面配置 JSON 顶层加一个独立的 `vision`
   段（`{"base_url":"…","api_key":"…","model":"…"（可选，缺省 `kimi-k3`）}`），
   跟主对话的 `provider`/`base_url`/`api_key` 完全独立、不做「provider 是
   kimi 就复用主 key」的隐式回退——即使两者都填 `kimi` 也各自查各自的。这条
   照抄 native 的先例：`agent-cli::main::resolve_vision` /
   `agent-server::bootstrap::resolve_vision` 从来都是从 `providers.toml` 里
   独立的 `[providers.kimi]` 段查，不看 `[default]` 是谁。解析函数是
   `vision::KimiVisionConfig::parse(config_json)`——**没有挂在
   `crate::config::HostConfig` 上**，因为 `config.rs` 不在本条允许改动的文件
   范围内；做法是对同一份 `config_json` 字符串再做一次独立的
   `serde_json::Value` 解析，只挑 `vision` 这个顶层键，`HostConfig::parse`
   那次解析会静默忽略这个多出来的键，两次解析互不干扰。`vision` 段缺失，或
   `base_url`/`api_key` 缺失/为空 → `parse` 返回 `None`，**不是构造期硬
   错误**（跟 `ProviderConfig::resolve_key()` 把空字符串当「没填」同一个
   宽容度）——页面可以先把 `AgentHost` 建起来，用户之后再补 Kimi key。

3. **没配 key → reject 不 panic**：`vision::inspect` 第一步就是
   `vision.ok_or_else(...)`，message 里含字面量 `not_configured`，措辞对齐
   `vision_inspect.rs` 的同名错误码。`bytes`/`mime`/`question` 全部是
   `wasm_bindgen` 原生支持的类型（`Vec<u8>` ↔ `Uint8Array` 是 wasm-bindgen
   内置转换），没有 `unwrap`/`expect` 在这条路径上。

**key 不泄漏**：`KimiVisionConfig` 不派生 `Debug`（跟 `HostConfig` 同一条
理由）；三条 reject 路径（not_configured / too_large / 网络失败）的 message
构造里都没有手写拼接 `api_key`，网络失败那条转发 `UploadError`/`TransportError`
的 `Display`，两者内部已经过 125/113 的 `redact`。

**没碰的文件**：`config.rs`（`HostConfig` 未改一行——`vision` 段走独立解析，
不是它的字段）、`Cargo.toml`（没新增 web-sys feature，`vision.rs` 只调
`agent_transport`/`agent_tools` 现成的 async 入口，没有直接碰任何 `web_sys`
类型）。

**编译期验收**：`bash scripts/build-wasm.sh --dev` 过。`cargo test --workspace`
全绿（本条没有可独测的纯逻辑增量——四步全部转发到 125/126 已经测过的函数，
`vision.rs` 里唯一的本地逻辑是 `KimiVisionConfig::parse` 的字符串抽取，判断
过于琐碎，没有为它单独派测试 agent，跟 issue 标注的「独测：真机」一致）。
`scripts/check-invariants.sh --all` 报的行数超限清单里没有本条改动的文件
（`host.rs` 276 行、`vision.rs` 160 行，均在 300 以内）。

**待真机清单**（本次未做，环境不允许）：

- [ ] 页面配置 JSON 加 `vision.base_url`/`vision.api_key`（真实 Kimi key），
      上传一张写着四位数字的图，调 `inspectImage`，确认返回文本答对数字
      （同 `probes/PROVIDERS.md §八` 的判据）。
- [ ] 传超过 2 MiB 的字节：确认 Promise reject 且 DevTools Network 面板
      **没有**出现任何新请求（含 Kimi 上传/chat 两个端点）。
- [ ] 配置 JSON 不带 `vision` 段（或带但 `api_key` 为空）：确认 reject，
      message 含 `not_configured`，页面不崩、不 panic。
- [ ] 上面三条 reject 的 message 文本里目视确认不含任何 key 片段。
- [ ] （可选，补测试覆盖）不填 `vision.model` 时确认请求体里 `model` 字段是
      `kimi-k3`；填了就用填的那个——验证 `DEFAULT_KIMI_MODEL` 的 fallback
      逻辑，可以在 DevTools Network 面板看请求体，不需要真的等 Kimi 回复。

## 真机验收（主会话，2026-08-12，Chrome via playwright MCP + 真 Kimi key）

**四条全过，主判据一次命中。**

### 主证据：模型答对了图里的数字

canvas 现画一张 640×360、白底黑字写着 **`7413`** 的 PNG（16442 字节），
调 `inspectImage(bytes, 'image/png', '这张图片里写着什么数字？只回答数字。')`：

```
返回："7413"          耗时 3998 ms
```

同 `probes/PROVIDERS.md §八` 当年验 Kimi 识图的判据。这一次证明的是**四步链路
在浏览器里完整跑通**：`upload_image_async`（113）→ `ms://` → `chat_body`（126）
→ `post_json_async`（125）→ `parse_content`（126）。

### 其余三条

| 验收 | 结果 |
|---|---|
| 没配 `vision` → reject 含 `not_configured` | ✅「not_configured：识图未配置——页面配置 JSON 缺少 vision.base_url / vision.api_key，没有默认 key（127 硬约束）」 |
| 超 2 MiB → reject **且零网络请求** | ✅ 边界精确：`limit+1 = 2097153` 字节被拒；`performance.getEntriesByType('resource')` 里 Kimi 域名的请求数 **2 → 2 未增**。（那 2 个正是上面成功那次的 upload + chat——**一次识图恰好两跳**） |
| reject 消息不含 key 片段 | ✅ 取 key 的第 8–24 字符做子串检查，三条 reject 全部不含 |

### ⚠️ 顺带发现一个没人认领的接缝（留给 130）

**`www/index.html` 的配置 JSON 里没有 `vision` 段**，所以**页面建出来的 `AgentHost`
永远是 `vision: None`，`inspectImage` 必然 `not_configured`**。

不是谁的 bug——127 的改动范围明确排除 `index.html`，129 的 issue 里没提 vision 配置。
本次真机是绕过页面、直接 `import('/pkg/agent_wasm.js')` 自建带 `vision` 段的宿主验的，
**Rust 侧完全正确**。

但 [130](130-browser-vision-end-to-end.md) 接端到端时**必须先把这一段补进页面**
（两个输入框：Kimi base_url + api_key，key 用 `type=password`，横幅只许显示长度），
否则模型调 `web:source/vision` 会稳定拿到 `not_configured`，而且看起来像识图坏了。
已在 130 的「做什么」里加了这一条。
