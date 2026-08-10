# 113 `agent-transport` 的 fetch 实现：浏览器侧比 ureq 那套薄

**里程碑** M13 · **依赖** 111 · **模型** sonnet · **独测** ✅（流式分帧错了会静默丢内容）

## 为什么这条不难

`agent-transport` 今天的四件事里，只有第一件要重做：

| 文件 | 干啥 | wasm 侧 |
|---|---|---|
| `client.rs` | `post_stream()`——本 crate 唯一的请求方法，只做流式 | 换 `fetch` + `ReadableStream` |
| `read_loop.rs`（165 行） | 读线程 + `mpsc::sync_channel`，为了在阻塞流上做中断 | **整个不需要**——`AbortController` 就是那个句柄 |
| `backoff.rs` | 连接期退避 + jitter | 逻辑照搬，纯计算 |
| `config.rs` | 解析 `providers.toml` | **不移植**，浏览器没有这个文件（配置来源见 114） |
| `upload.rs` | 图片上传到供应商 | `fetch` 直接可用 |

`read_loop.rs` 的存在理由写在 `lib.rs` 顶部：ureq 的阻塞 `read` 没有外部中断句柄，022 第一版
用短 socket timeout 当轮询、023 发现慢首字节的家（Kimi）会被误判成死连接，才改成读线程 +
双超时旋钮，自称「不优雅但可测」。**这笔债在浏览器侧不用还**。

## 范围

1. 按目标平台分实现：native 保留 ureq 那条一行不动，wasm 走 fetch。
   接缝形状由实现者定，但**上层看到的必须是同一个 `Client::post_stream`**——
   `agent-providers` 与 `agent-runtime` 一行都不该因为换平台而改。
2. 取消语义对齐：native 那条的取消标志轮询，wasm 侧换成 `AbortController::abort()`。
   **对上层的可观测行为必须一致**（同样的取消点、同样的错误分类）。
3. 错误分类**仍然不在这里做**（`lib.rs` 已写明：transport 不知道自己在跟哪家说话）。
   非 200 一律打包成 `TransportError::Http` 交上层。

## 验收（可判定）

- native 侧：`cargo test -p agent-transport` 全绿，且与本 issue 之前**逐字相同**的行为
  （录制帧测试不变）。
- wasm 侧：用录制帧喂进 fetch 实现，**分帧结果与 native 实现逐字节相同**。这是核心断言——
  流式分帧错了不会报错，只会静默丢内容或粘帧。
- 取消：wasm 侧发起请求后立即 abort，上层拿到的错误分类与 native 侧取消时**同一档**。
- `rg ureq crates/` 仍**只命中 `agent-transport`**（`lib.rs` 那条镜像约束不许破）。

## 注意

- 三家的流式差异已经在 `probes/PROVIDERS.md` 里结论化过，**别在本 issue 里重新探**——
  照那份结论实现，对不上再回头改探针。
- `upload.rs` 的图片上传路径（M11）不要在本 issue 里顺手改语义，只做平台适配。
