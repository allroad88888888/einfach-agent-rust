# 084 transport 的图片上传

> ⚠️ **已废弃（superseded by s5）**：本文描述的 images 管线（`ContentBlock::Image` /
> `POST /files` / `upload_base_url` / `ImagesDropped` / 前端选图 / vision 子 agent 委托）
> 已被 s5 重构整体移除，现以 `POST /uploads` 上传端点 + `srv:vision/inspect` 工具取代。
> 正文仅作历史决策档案保留，不再反映当前实现。

**里程碑** M11 · **依赖** — · **模型** sonnet · **独测** ✅（错误分类要判断）· **状态** 完成

**先读 [docs/IMAGES.md](../IMAGES.md) §二**（官方接口与限制）。

**无依赖，可以立刻开工**，跟 [079](079-image-content-block.md)、
[080](080-adjustment-images-dropped.md) 并行。自包含：一个上传函数 + 对着假 server 的单测。

## 范围

`crates/agent-transport/` 加一个上传调用（IO 归这个 crate，红线 7）：

1. **`POST {base_url}/files`**，multipart form，字段 `file` + `purpose: "image"`。
   响应体里的 `id` 就是引用要用的那个。
2. **`ms://` 前缀在这里拼**。transport 是知道自己在跟哪家说话的那一层；
   往上交出去的是一个**完整引用字符串**，core 拿到它时已经不透明了（IMAGES.md §七）。
3. **官方限制照抄，不自己发明**：单文件 ≤100MB，请求体 ≤100MB，
   推荐图 ≤4K（4096×2160）。超限**在发之前就拦下**并给出可读的错误，别打出去等 400。
4. **错误要能分类**：文件太大 / key 不对 / provider 拒了 / 网络断了——
   照 `agent-transport` 现有的错误形状走，别新发明一套。

## 验收（可判定）

1. **假 server 打通**：起一个假 HTTP server 收 multipart、回一个 `{"id":"..."}` →
   函数返回 `ms://<那个 id>`，**逐字符相同**。
2. **multipart 形状对**：假 server 断言收到的 `purpose` 字段是 `image`、
   文件字节与传入的**逐字节相同**（别被编码吃掉）。
3. **超限不发**：传一个 >100MB 的东西 → **假 server 一个请求都没收到**，
   直接返回错误。断言的是「没发出去」，不是「返回了错误」。
4. **错误分得开**：假 server 分别回 401 / 413 / 500 → 三种错误可区分。
5. **key 不外泄**：任何错误报文、任何 `Debug` 输出里都不出现 api_key
   （只出长度/状态）。

## 突变验证（必做）

- 把超限检查删掉 → 第 3 条必须变红。
- 把 `ms://` 前缀拼错（比如漏掉）→ 第 1 条必须变红。

## 注意

- **红线 7**：core / store 一行 IO 都不许有。本 issue 全部改动在 `agent-transport`。
- **红线 8**：假 server 绑 `127.0.0.1`。
- **假 server 的坑（077 踩过，别再踩）**：`accept()` 出来的 socket 在 BSD/macOS 上
  **继承** listener 的 `O_NONBLOCK`（Linux 不会）。accept 之后**必须
  `set_nonblocking(false)`**，否则高负载下会读到 `WouldBlock` 当成空请求 →
  测试随机假红。现成先例：`agent-transport/tests/fake_sse.rs:260`。
- **providers.toml 只读不印不提交**；真机试打时任何输出只出长度/状态。
- 真机试打要加 `--noproxy '*'`（本机 `http_proxy=127.0.0.1:7897`，不加会假 502）。
- 收工验证前台跑完，含 `--features ts`。

---

## 实做记录（完成 · 2026-08-04）

- `agent-transport` 新增 `ImageUpload`、`UploadError`、`MAX_IMAGE_BYTES` 与
  `Client::upload_image`。它在发出请求前分别检查原文件和 multipart body 的 100 MiB
  上限，向 `{base_url}/files` 发送 `purpose=image` 和未改动的二进制 `file`，仅在
  transport 拼接完整的 `ms://<id>` 引用。
- 两个 loopback HTTP 黑盒测试分别断言 multipart 的请求行、字段、原始字节和完整引用，
  以及超限零请求、401/413/500 分类、畸形响应/网络错误与 `Display`/`Debug` 的 key 脱敏。
  `cargo test -p agent-transport --test file_upload_success --test file_upload_failures`：4 passed，
  0 failed。
- 这台机器设置了 `http_proxy`，而 ureq 2 会连 `127.0.0.1` 也走它，令假 server 等到读超时。
  transport 的 `AgentBuilder` 现在显式 `try_proxy_from_env(false)`，使本地和上游请求不再被
  进程环境的代理静默改道；fake server 仍严格绑定 `127.0.0.1`，每个 accepted socket 都复位为
  blocking。

### 突变验证：先红后恢复

临时删除两个上传前大小检查，运行
`cargo test -p agent-transport --test file_upload_failures oversize_image_is_rejected_before_any_http_request -- --exact`。
最初的 300ms listener 窗口会在 100 MiB multipart 完成前结束，只会得到网络错误，不能验证
「没有发出去」；已改为 3 秒观察窗口。随后该目标断言首先红为（实现已恢复）：

```text
thread 'oversize_image_is_rejected_before_any_http_request' (42446838) panicked at crates/agent-transport/tests/file_upload_failures.rs:95:5:
assertion `left == right` failed: 超过大小限制时不得连到假 server
  left: 1
 right: 0
note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace
```

临时把 `Ok(format!("ms://{}", response.id))` 改为 `Ok(response.id)`，运行
`cargo test -p agent-transport --test file_upload_success uploads_expected_multipart_and_returns_complete_ms_reference -- --exact`。
完整引用断言红为（实现已恢复）：

```text
thread 'uploads_expected_multipart_and_returns_complete_ms_reference' (42449711) panicked at crates/agent-transport/tests/file_upload_success.rs:164:5:
assertion `left == right` failed
  left: "file-abc123"
 right: "ms://file-abc123"
note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace
```

全 workspace 的 `cargo test`、`cargo test --features ts`、`cargo clippy --all-targets` 与
`scripts/check-invariants.sh --all` 留待 079–087 合并后的主会话收工验证。
