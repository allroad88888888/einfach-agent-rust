# 083 三家接线与降级告警

**里程碑** M11 · **依赖** [080](080-adjustment-images-dropped.md) + [082](082-image-array-encoding.md) · **模型** sonnet · **独测** ✅（**静默丢图**是本 issue 的头号风险） · **状态** 完成

**先读 [docs/IMAGES.md](../IMAGES.md) §一**（实测事实）**与 §三-6**（不许静默丢图）。

纯函数层，零 IO。**这一条是整个 M11 里最容易悄悄坏掉的**：丢了图功能全对、
回答还挺像样，用户永远发现不了。

## 范围

1. **三家各自表态**（判断落在各家 `encode` 里，红线 12）。实测取值
   （PROVIDERS.md §八，两轮不同 nonce 数字一致）：

   | | 吃图 | 拒了之后的原文 |
   |---|---|---|
   | `kimi` | **✓** | — |
   | `deepseek` | ✗ | `400 unknown variant 'image_url', expected 'text'` |
   | `glm` | ✗ | `400 1210 messages.content.type 参数非法，取值范围 ['text']` |

2. **吃不下的家：图片块编成占位文本**，让模型知道有这么个东西、但自己看不见。
   例如 `[用户上传了图片 发票.png（image/png），当前模型看不到图片内容]`。

   - **占位文本会进 prompt，只能由这个块自己的字段推出来**——不许有时间戳、
     计数器、随机 id、也不许依赖它在历史里的下标（红线 11）。
   - `name` 为 `None` 时的措辞**也要定死**，写进代码注释。

3. **同时报 `Adjustment::ImagesDropped { count }`**（080 加的变体），
   `count` 等于这一次请求里被降级的图片**总张数**。

## 验收（可判定）

1. **Kimi 走数组**：带图的料喂给 kimi 的 `encode`，body 里有 `image_url`，
   `url` 与 `reference` 逐字符相同。
2. **两家一个 `image_url` 都不许漏出去**：同一份料喂给 deepseek / glm 的 `encode`，
   产出的 `body` 里 **`grep` 不到 `image_url`，也 `grep` 不到 `reference` 那个字符串**。
   （这条同时兑现 IMAGES.md §七最后一段：换 provider 不会拿它去撞 400。）
3. **降级必报**：上面两家的 `Encoded.adjustments` 里有
   `ImagesDropped { count: N }`，N 等于真实张数（两张图就断言 2，别只断言「非空」）。
4. **Kimi 不误报**：kimi 的 `adjustments` 里**没有** `ImagesDropped`。
5. **占位文本逐字节确定**（红线 11）：同一个块编两次产出完全相同；
   把同一张图放在历史里不同位置，占位文本**不变**。
6. **无图 = 三家逐字节回到 082 之后的状态**（不带图的会话一个字节都不该动）。

## 突变验证（必做）

- **把 `ImagesDropped` 那句 push 删掉 → 第 3 条必须变红。**
  这条最重要，它挡的就是静默丢图。
- 把占位文本里塞一个计数器 → 第 5 条必须变红。
- 把 deepseek 的「吃图」改成 `true` → 第 2 条必须变红。

三条都真跑红一次，报文抄进实做记录。**构造得挡得住**：删掉 push 之后如果别的断言
先红了，等于这条护栏没验到，换种构造重来（这是 074 踩过的坑）。

## 注意

- **别在 `wire/messages.rs` 里 `match provider`**（红线 12）——那个文件三家共用，
  082 已经把口子开好了，这里只传值。
- 别改 PROVIDERS.md §八 的结论；真跑出不一样的数字 → **单列新 issue**，
  别默默改文档（那是实测记录，不是配置）。
- 收工验证前台跑完，含 `--features ts`。

## 实做记录

- `kimi::encode` 明确传入 `SUPPORTS_IMAGES = true`；`deepseek::encode` 与
  `glm::encode` 明确传入 `false`，并各自在本 adapter 内把共享 encoder 返回的
  `dropped_images` 转成 `Adjustment::ImagesDropped`。共享 wire 层没有 provider
  分支。
- 不支持图片时，命名图片固定为
  `[用户上传了图片 <name>（<mime>），当前模型看不到图片内容]`；无名图片固定为
  `[用户上传了图片（<mime>），当前模型看不到图片内容]`。措辞和顺序只由块自身字段
  决定。
- 独测（实现恢复后）：`cargo test -p agent-providers --test image_provider_fallback`，
  4 passed；覆盖 Kimi 两图 `image_url`、DeepSeek/GLM 零引用泄漏和精确计数、历史位置
  无关的占位文本，以及无图字符串形状与字节确定性。
- 突变一：删掉 DeepSeek 的 `Adjustment::ImagesDropped` push 后运行
  `cargo test -p agent-providers --test image_provider_fallback deepseek_and_glm_hide_refs_and_report_every_dropped_image -- --exact`，目标计数断言首先变红：

  ```text
  thread 'deepseek_and_glm_hide_refs_and_report_every_dropped_image' (42576224) panicked at crates/agent-providers/tests/image_provider_fallback.rs:110:9:
  assertion `left == right` failed: deepseek 降级两张图片必须精确报告两张
    left: []
   right: [ImagesDropped { count: 2 }]
  ```

- 突变二：在降级文本后追加历史下标计数器后运行
  `cargo test -p agent-providers --test image_provider_fallback fallback_placeholder_is_deterministic_and_independent_of_history_position -- --exact`，目标确定性断言首先变红：

  ```text
  thread 'fallback_placeholder_is_deterministic_and_independent_of_history_position' (42581680) panicked at crates/agent-providers/tests/image_provider_fallback.rs:138:5:
  assertion `left == right` failed: 占位文本只能由图片块字段决定，不能依赖历史位置
    left: String("附件\n[用户上传了图片（image/png），当前模型看不到图片内容] #0")
   right: String("附件\n[用户上传了图片（image/png），当前模型看不到图片内容] #1")
  ```

- 突变三：把 DeepSeek 的 `SUPPORTS_IMAGES` 改为 `true` 后运行
  `cargo test -p agent-providers --test image_provider_fallback deepseek_and_glm_hide_refs_and_report_every_dropped_image -- --exact`，目标零泄漏断言首先变红：

  ```text
  thread 'deepseek_and_glm_hide_refs_and_report_every_dropped_image' (42586872) panicked at crates/agent-providers/tests/image_provider_fallback.rs:103:9:
  deepseek 不得把 image_url 或不透明引用漏进请求
  ```
