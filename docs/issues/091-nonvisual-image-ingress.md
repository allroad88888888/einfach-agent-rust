# 091 非视觉 provider 在 adapter 降级前被上传短路

> ⚠️ **已废弃（superseded by s5）**：本文描述的 images 管线（`ContentBlock::Image` /
> `POST /files` / `upload_base_url` / `ImagesDropped` / 前端选图 / vision 子 agent 委托）
> 已被 s5 重构整体移除，现以 `POST /uploads` 上传端点 + `srv:vision/inspect` 工具取代。
> 正文仅作历史决策档案保留，不再反映当前实现。

**里程碑** M11 补充 · **依赖** [083](083-image-provider-fallback.md) + [085](085-http-image-ingress.md) + [087](087-image-dogfood.md) · **模型** opus · **独测** 浏览器 + provider mock · **状态** 完成

由 087 真机续跑发现。083 已规定 DeepSeek/GLM 的图片必须变成占位文本并产生
`ImagesDropped`，但 HTTP 图片入口目前会先将每张图上传到当前 provider 的 `upload_base_url`，然后才
dispatch 到 adapter。非视觉 provider 若没有兼容 `/files` 上传端点，流程将在生成占位文本和告警前失败，
用户看不到这条关键降级提示。

## 现象与证据

图片输入路由无条件调用 `upload_image(upload_base_url, ...)`；adapter 的
`history_with_image_support` 则在这之后才会为 DeepSeek/GLM 生成占位文本和 `ImagesDropped`。087 尝试用
不落盘的特殊文件描述符覆盖 provider，loader 未采用它，启动横幅仍为 Kimi；该次调用没有作为
DeepSeek/GLM 真机证据。本 issue 要先让非视觉链路可抵达 adapter，再做有效 dogfood。

## 范围

确定 HTTP ingress、图片引用生命周期和 adapter capability 之间的正确接缝，使不支持视觉的 provider
无需先命中其文件上传 API 也能把图片输入交给 083 的降级逻辑。provider-specific 文本/占位规则仍只在
adapter；不得在共用 wire 编码层 `match provider`，也不得静默丢图。

## 验收（可判定）

1. mock 非视觉 provider 设置不可访问的上传端点，发送图片仍能到达 adapter；测试断言没有对该端点发
   HTTP 请求、模型输入含 083 规定的占位文本、turn guard 含 `ImagesDropped`。
2. 浏览器连接可选的真实 DeepSeek 或 GLM 会话，贴随机数字图：模型明确表示看不到图片内容，界面同时
   显示 `ImagesDropped` 告警；两项缺一不可。
3. Kimi 的视觉路径仍按 088 使用独立 `/v1/files`，并能发出 `ms://` 引用；无图请求逐字节保持
   `{"text":"..."}`。
4. 上传大小、mime、失败分类和 085 的「上传不进 provider_call actor」约束仍有真实断言。

## 不在范围

- 不给 DeepSeek/GLM 伪造视觉能力或调用未经验证的文件 API。
- 不改 `probes/PROVIDERS.md`，不打印、修改或提交 `providers.toml`/api_key。
- 不修 089 的缓存差或 090 的 UI 回放。

## 注意

- 先读 [IMAGES.md](../IMAGES.md) 的决定 3、4 及红线 12，和 [INVARIANTS.md](../INVARIANTS.md) 第 7、9、11、12 条。
- 真机 provider 调用必须串行，避免重复计费；若配置选择机制本身缺失，另列 issue，不把临时密钥复制进测试。

## 实做记录（完成 · 2026-08-05）

- `Provider` 新增默认保守的 `supports_images()`；仅 Kimi 覆盖为 `true`。HTTP ingress 在
  dispatch 前据此选择：视觉 provider 保留 085 的先上传路径，非视觉 provider 只校验 MIME/大小，构造
  不可解析的内部图片引用。core 仍只保存不透明 `reference`，083 的 adapter 继续是唯一生成占位文本与
  `ImagesDropped` 的位置。
- 新增 `nonvisual_image_input` 集成测试，用真实 HTTP/SSE server 和 DeepSeek adapter mock 验证：上传 URL
  不可访问时请求仍为 `202`；上游只收到 `/openai/v1/chat/completions`，完全没有 `/files`；模型最后一条
  内容精确为 `请检查附件\n[用户上传了图片 receipt.png（image/png），当前模型看不到图片内容]`；终局
  guard 精确为 `ImagesDropped { count: 1 }`。测试也确认输入图片不会进入 provider-call actor。
- 收尾复核为 input 路由配置了有限请求体上限，并在任何上传前限制每轮最多 8 张图片、累计原始图片
  字节不超过 100 MiB；真实 HTTP 回归请求超过 axum 默认 2 MiB，仍能到达 handler 和 adapter。
- 定向验证：`nonvisual_image_input` 2/2、085 的超大附件边界校验单测 1/1、三家 adapter 图片编码测试
  4/4 均通过；088 的 Kimi `/v1/files` + `ms://` 和无图文本 JSON 断言仍是其独立测试的覆盖。
- 真机 DeepSeek：以前台同源 server 和真实 Chrome 贴入 29,878-byte 的随机数字图（图中为 **6636**）后，
  模型最终精确回复 `我看不到图片内容`，同一轮 UI 精确显示
  `adjustments=ImagesDropped{\"count\":1}`。这次只发出一笔真实模型请求；临时配置和图片已移至废纸篓，
  未读取、输出、修改或提交 `providers.toml`。

### 实际踩坑

- SSE 连接本身常驻，浏览器 dogfood 不能等待 `networkidle`；必须以 DOM 就绪和状态文本为准。首次浏览器
  脚本还把 `ImagesDropped{\"count\":1}` 的匹配字符串多转义了一层，导致等待超时；同一页已经显示上面的
  模型回复与 guard，因此没有为脚本修正而重复计费调用。

### 突变验证

将非视觉 ingress 构造的图片名故意改为 `None`，使 adapter 收不到图片元数据。对应的占位文本护栏首先
变红，原始输出如下：

```text
running 1 test

thread 'nonvisual_image_reaches_adapter_without_an_http_upload' (44141663) panicked at crates/agent-server/tests/nonvisual_image_input.rs:40:5:
assertion `left == right` failed: 非视觉 adapter 必须拿到图片元数据并生成 083 的确定性占位文本
  left: String("请检查附件\\n[用户上传了图片（image/png），当前模型看不到图片内容]")
 right: String("请检查附件\\n[用户上传了图片 receipt.png（image/png），当前模型看不到图片内容]")
note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace
test nonvisual_image_reaches_adapter_without_an_http_upload ... FAILED

failures:

failures:
    nonvisual_image_reaches_adapter_without_an_http_upload

test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.03s

error: test failed, to rerun pass `-p agent-server --test nonvisual_image_input`
```

恢复图片名后，`cargo fmt --check` 与该集成测试再次通过。
