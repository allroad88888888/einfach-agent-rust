# 088 Kimi 真机图片上传端点

**里程碑** M11 补充 · **依赖** [084](084-transport-files-upload.md) + [085](085-http-image-ingress.md) · **模型** 主会话真机 · **独测** 真 Kimi · **状态** 完成

由 [087](087-image-dogfood.md) 发现；这是上传配置/端点的独立问题，不在 dogfood 中硬修。

## 现象

2026-08-04，以同源静态 server、真实浏览器和默认 Kimi 会话发送一张由 `probes/api` 生成的 PNG。
前端先显示本地缩略图，随后精确报：

```text
出错：bad_request: 图片上传被服务商拒绝（HTTP 404）
```

同一会话紧接着的纯文本请求获得 Kimi 回复 `TEXT-OK`，说明会话和聊天请求可用；图片请求尚未进入
模型（没有 thinking、answer 或 guard），因而不能把它归为模型视觉能力问题。`providers.toml` 未打印、
修改或提交。

## 范围

确认 Kimi 的聊天 endpoint 与图片上传 endpoint 的正确、可配置关系，使 HTTP 图片入口使用正确目标，
并保持 084 的 `ms://<id>` 不透明引用契约。以官方图片 API 决策为准，不猜测也不泄露本地配置或 key。

## 验收（可判定）

1. 有 mock 覆盖聊天 endpoint 与上传 endpoint 可不同：上传请求精确到正确的 `/v1/files` 路径，
   multipart、`purpose=image` 与 `ms://` 返回值仍满足 084。
2. 真实 Kimi、真实浏览器发送一张嵌入随机四位数的 PNG：上传不返回 404，服务端接收完整 `ms://`
   引用，模型回复包含该四位数。
3. 不带图的请求体仍逐字节为 `{"text":"..."}`，同一会话纯文本仍可完成。
4. 失败分类仍可读且任何响应、日志、`Display`/`Debug` 均不含 api_key。

## 不在范围

- 不重做 087 的 undo/redo、多图或降级验收；上传修复后回到 087 真机记录续跑。
- 不改 `probes/PROVIDERS.md`，不把 API key 或 providers 配置写进测试、日志或提交。

## 实做记录（2026-08-05）

- `SessionTemplate` 新增独立的 `upload_base_url`；bootstrap 从 provider 的 API base 填入它，
  聊天仍使用完整 `endpoint`。HTTP 图片入口只把 `upload_base_url` 交给 transport，后者继续
  负责追加 `/files`。因此不会从聊天路径反推上传路径。
- mock 把聊天设为 `/openai/v1/chat/completions`、上传设为 `/v1/files`。图片测试精确断言后者、
  `purpose=image`、文件 multipart 字段和 `ms://` 引用；同一测试还精确断言无图请求仍是
  `{"text":"..."}`。相关四个 server 测试二进制共 10 条通过。
- 真机同源静态 server 的启动横幅为 `provider=kimi model=kimi-k3`。随机探针图（29,878 bytes）
  的数字为 **9682**；浏览器贴图后 Kimi 回复包含 `9682`，不再出现 HTTP 404。随后纯文本
  `只回复 TEXT-OK，不要解释。` 的回复精确为 `TEXT-OK`。全过程没有打印、修改或提交
  `providers.toml`。

### 实际踩坑

`provider_cfg.endpoint()` 已经是完整的 `/chat/completions` 路径。把它交给会追加 `/files` 的
transport 会请求 `/chat/completions/files`，Kimi 返回 404；上传 API base 必须是显式字段，不能
由聊天 endpoint 作字符串推导。

### 突变验证

将 `routes/input.rs` 中上传用的 `template.upload_base_url.clone()` 故意改成
`template.endpoint.clone()` 后，运行：

```text
cargo test -p agent-server --test http_image_input text_stays_on_the_old_wire_shape_and_uploaded_reference_survives_recovery -- --exact
```

对应护栏的原始红灯如下：

```text
   Compiling agent-server v0.1.0 (/Volumes/work/self/einfach-agent-rust/crates/agent-server)
    Finished `test` profile [unoptimized + debuginfo] target(s) in 27.68s
     Running tests/http_image_input.rs (target/debug/deps/http_image_input-3b6f3bf139115ba0)

running 1 test
test text_stays_on_the_old_wire_shape_and_uploaded_reference_survives_recovery ... FAILED

failures:

---- text_stays_on_the_old_wire_shape_and_uploaded_reference_survives_recovery stdout ----

thread 'text_stays_on_the_old_wire_shape_and_uploaded_reference_survives_recovery' (43812606) panicked at crates/agent-server/tests/http_image_input.rs:61:5:
assertion `left == right` failed: 上传必须走独立的文件端点，不能把聊天路径继续追加 /files
  left: "/openai/v1/chat/completions/files"
 right: "/v1/files"
note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace


failures:
    text_stays_on_the_old_wire_shape_and_uploaded_reference_survives_recovery

test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.04s

error: test failed, to rerun pass `-p agent-server --test http_image_input`
```

恢复实现后相同命令：`1 passed; 0 failed`。
