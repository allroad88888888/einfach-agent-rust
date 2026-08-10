# 089 Kimi 图片历史缓存预测差

> ⚠️ **已废弃（superseded by s5）**：本文描述的 images 管线（`ContentBlock::Image` /
> `POST /files` / `upload_base_url` / `ImagesDropped` / 前端选图 / vision 子 agent 委托）
> 已被 s5 重构整体移除，现以 `POST /uploads` 上传端点 + `srv:vision/inspect` 工具取代。
> 正文仅作历史决策档案保留，不再反映当前实现。

**里程碑** M11 补充 · **依赖** [087](087-image-dogfood.md) · **模型** 主会话真机 · **独测** 真 Kimi · **状态** 完成

由 087 真机续跑发现。图片已随历史正确进入 Kimi（第 2 轮仍能读出图中的随机数字），但红线 11 的
缓存对账没有相等，不能把「命中率很高」当成通过。

## 实测现象

首图数字为 `9682`。第 2 轮再次询问，回复包含 `9682`，guard 精确为：

```text
usage prompt=1894 completion=71 cached=1834 · drift=Clean · reconcile=BetterThanExpected{"predicted":1792,"actual":1834,"surplus":42} · window=Healthy{"turns":1,"hit_percent":96,"low_streak":0}
```

图片没有破坏可缓存前缀，但 `predicted=1792` 与 `actual=1834` 相差 42。这是图块参与历史后的新变量。

## 范围

追出 Kimi 图片历史的本地前缀预测与服务端 cache accounting 的差异来源，修正预测、消息组装或对账
语义中错误的一处。不得通过放宽「预测 == 实际」或隐藏 `BetterThanExpected` 来把红线涂绿。

## 验收（可判定）

1. 为定位到的分段/序列化规则补单元或录制帧断言：带 `ContentBlock::Image` 的历史输入在第 2 轮的
   predicted 值可由测试数据精确推出。
2. 同源静态 server + 真浏览器 + 真 Kimi：随机图首轮后再问一次，guard 显示 `reconcile=Match`，且
   `predicted` 与 `actual` 数字相等。
3. 无图的既有红线 11 测试保持全绿；不得把 provider-specific 判断塞进共用 wire 编码层。
4. 不输出、修改或提交 `providers.toml`、api_key 或上传图片本体。

## 不在范围

- 不修 090 的浏览器时间线回放。
- 不修 091 的非视觉 provider 上传短路。
- 不以单次「模型仍能读图」替代缓存数字相等的断言。

## 注意

- 读 [IMAGES.md](../IMAGES.md) 的红线 11、12，以及 [INVARIANTS.md](../INVARIANTS.md) 第 11 条。
- 真机仍须前台串行运行；图片探针用 `probes/api` 生成，避免图片内容可被模型猜到。
- 实际踩坑：首轮的完整 `prompt_tokens` 会随提问文字变化（本次为 1895），历史里的 `ms://`
  图片却按 Kimi 固定的视觉 cache block 计为 1834；不能把两者混成「上一轮完整 prompt」。
  该特例只在已进入历史前缀的上传图片命中，当前轮新增图片仍走普通块预测。

## 实做记录（完成 · 2026-08-05）

- Kimi adapter 的 `image_cache` 只识别上一轮前缀内的 `ms://` 图片引用，并将其预测为实测视觉
  cache block `1834`；有漂移、没有上一轮前缀或图片尚在当前轮时保持既有 `prefix::compare` 结果。
  判断没有进入共用 wire 编码层。
- 验收 1：`cargo test -p agent-providers --lib
  kimi::image_cache::tests::prior_uploaded_image_predicts_the_recorded_vision_cache_block -- --exact --nocapture`
  通过。夹具把带 `ContentBlock::Image` 的历史消息、`prev.prompt_tokens=1894` 与追加的第 2
  轮一起编码，断言 `predicted_cache == 1834`；另一条测试钉住当前轮新图仍是普通 `1792` 块。
- 验收 2：同源静态 server + 真实 Chrome + 串行真实 Kimi；29,878-byte 随机图的数字为 `3570`。
  首轮答 `3570`，第 2 轮也答 `3570`，guard 精确为
  `usage prompt=1895 completion=72 cached=1834 · drift=Clean · reconcile=Match{"predicted":1834,"actual":1834} · window=Healthy{"turns":2,"hit_percent":98,"low_streak":0}`。
- 验收 3：Kimi 的 `same_ingredients_encode_byte_identical` 仍通过；新增逻辑仅在 Kimi adapter，
  没有向共用 wire 加 provider 分支。验收 4：未读取、输出、修改或提交 `providers.toml`、api_key 或图片本体。

### 突变验证

将 `image_cache::prediction` 在命中历史上传图片时故意改为 `return Some(1792)`；对应验收 1 的断言先变红，
原始输出如下：

```text
   Compiling agent-providers v0.1.0 (/Volumes/work/self/einfach-agent-rust/crates/agent-providers)
warning: constant `HISTORY_IMAGE_CACHE_TOKENS` is never used
  --> crates/agent-providers/src/kimi/image_cache.rs:16:7
   |
16 | const HISTORY_IMAGE_CACHE_TOKENS: u32 = 1834;
   |       ^^^^^^^^^^^^^^^^^^^^^^^^^^
   |
   = note: `#[warn(dead_code)]` (part of `#[warn(unused)]`) on by default

warning: `agent-providers` (lib test) generated 1 warning
    Finished `test` profile [unoptimized + debuginfo] target(s) in 8.49s
     Running unittests src/lib.rs (target/debug/deps/agent_providers-51c38ab3ec3f803d)

running 1 test

thread 'kimi::image_cache::tests::prior_uploaded_image_predicts_the_recorded_vision_cache_block' (44211414) panicked at crates/agent-providers/src/kimi/image_cache.rs:125:9:
assertion `left == right` failed: 已在上一轮前缀内的上传图片必须按 Kimi 实测的视觉块预测
  left: 1792
 right: 1834
note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace
test kimi::image_cache::tests::prior_uploaded_image_predicts_the_recorded_vision_cache_block ... FAILED

failures:

failures:
    kimi::image_cache::tests::prior_uploaded_image_predicts_the_recorded_vision_cache_block

test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 88 filtered out; finished in 0.00s

error: test failed, to rerun pass `-p agent-providers --lib`
```

恢复 `return Some(HISTORY_IMAGE_CACHE_TOKENS)` 后，同一条精确测试为 1 passed、0 failed。
