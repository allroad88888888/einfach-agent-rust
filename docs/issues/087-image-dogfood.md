# 087 图片附件真机 dogfood ← M11 终点

> ⚠️ **已废弃（superseded by s5）**：本文描述的 images 管线（`ContentBlock::Image` /
> `POST /files` / `upload_base_url` / `ImagesDropped` / 前端选图 / vision 子 agent 委托）
> 已被 s5 重构整体移除，现以 `POST /uploads` 上传端点 + `srv:vision/inspect` 工具取代。
> 正文仅作历史决策档案保留，不再反映当前实现。

**里程碑** M11 · **依赖** 079–086 全部 · **模型** 主会话真机 · **独测** 真机 · **状态** 完成

M11 的「能用」终点。照 M6/M8/M9/M10 的规矩：**验收靠一次真实运行，不靠形容词。**

## 跑法

`AGENT_STATIC_DIR` 由 server 同源托管 `packages/web/dist`（**不经 vite dev 代理**，
省掉一个变量——M8/M10 dogfood 的做法），playwright 驱动真浏览器，真 Kimi。

**图里必须埋一个模型没处猜的东西。** `probes/api` 现成：

```
cd probes/api && cargo run --bin multimodal -- --nonce <随便> --dump /tmp/probe.png
```

它印一个四位数、不打任何 API。**200 不等于看见**——这条必须是行为级断言
（E9 在消息级 system 上踩过同一个坑：收 ≠ 听）。

## 验收（可判定）

**一、模型真的看见了**
贴那张图 + 问「图里印着什么数字」→ 回答里出现**那四位数**。

**二、红线 11**
第 2 轮起缓存对账 `预测 == 实际`。图片进了历史但**没有破坏前缀稳定性**。
（M6/M8/M10 都验过这个形式，图片是新变量，要重新确认一次。）

**三、undo / redo**
`/undo` 撤掉带图那一轮 → 时间线上那张图消失；`redo` → 图回来，
**且下一轮模型仍然看得见它**（不是只有图标回来了——要再问一次那四位数）。

**四、降级可见**（**M11 最容易悄悄坏掉的一条**）
同一份前端连一个 **DeepSeek 或 GLM** 的会话，贴同一张图 →
模型明确说自己看不到图片内容，**且界面上能看到 `ImagesDropped` 那条告警**。
两个都要：只有前者说明模型收到了占位文本，只有后者说明系统没瞒着用户。

**五、不选图 = 老路不变**
纯文本发一句，行为与 M11 之前完全相同。

**六、多张图**
一次贴两张不同数字的图 → 模型两个数字都答得出来，顺序不乱。

## 注意

- **真机步骤前台跑完、如实报**（WORKFLOW §四 -1）。
- **真机若捞到新问题 → 单列新 issue**，不塞进本 issue 硬修
  （049/050/060/068/078 的先例）。
- **providers.toml 只读不印不提交**，任何输出只出长度/状态。
- `curl` 一律加 `--noproxy '*'`（本机 `http_proxy=127.0.0.1:7897`，不加会假 502，
  M9 踩过还一度被误判成「vite 代理坏了」）。
- 跑完把逐条证据（不是「跑通了」这种形容词）写进本文件的「真机记录」一节，
  照 068 的格式。

---

## 真机记录（阻塞 · 2026-08-04）

前台启动同源静态 server，并以 Playwright 操作真实浏览器和默认 Kimi 会话；没有输出、修改或
提交 `providers.toml`。探针生成的首张 PNG 为 29,878 bytes，命令输出的图内数字为 **8516**。

| 验收 | 真机操作与观察 | 结果 |
|---|---|---|
| 一、模型看见图片 | 选择该 PNG，输入「请读取这张程序生成图片里的四位阿拉伯数字。只回复这四个数字，不要解释。」并发送。浏览器时间线先显示本地缩略图和文件名。随后状态精确显示 `出错：bad_request: 图片上传被服务商拒绝（HTTP 404）`。没有 Kimi 的 thinking、answer 或 guard。 | **阻塞**：请求在上传阶段失败，未进入模型，不能用 200/本地缩略图冒充读图证据。 |
| 二、红线 11 | 首图未形成 server history，故没有可用于第 2 轮的图片前缀及缓存对账。 | **阻塞**。 |
| 三、undo / redo | 首图上传失败，无法拥有一个有效的带图历史轮次。 | **阻塞**。 |
| 四、降级可见 | 相同图片在 Kimi 上传阶段即被拒绝；未把图送到 provider，不能误报 DeepSeek/GLM 的占位文本或 `ImagesDropped`。 | **阻塞**。 |
| 五、不选图老路 | 同一真实 Kimi 会话发送 `只回复 TEXT-OK，不要解释。`。模型最终回复精确 `TEXT-OK`，其 guard 为 `usage prompt=1788 completion=44 cached=— · drift=Clean · reconcile=Blind{"predicted":0} · window=NoData{"skipped":1}`。 | **完成**。 |
| 六、多张图 | 首张图已经被 404 阻断，未发送两图以避免把同一已知上传失败重复计为模型证据。 | **阻塞**。 |

已新建 [088](088-kimi-upload-endpoint.md) 记录可复现的上传端点问题。依照本 issue 的边界，未在
dogfood 中修改 transport、配置或 provider 逻辑；088 修复并重跑真实 Kimi 后，才可继续本 issue 的
五项图片验收。

---

## 真机记录（续跑 · 2026-08-05）

088 修复后，前台启动同源静态 server，并以 Playwright 控制真实浏览器和默认 Kimi 会话。启动横幅为
`provider=kimi model=kimi-k3`。未输出、修改或提交 `providers.toml`。两张程序生成的 PNG 均为
29,878 bytes，图内随机数字依次为 **9682**、**8799**。

| 验收 | 真机操作与观察 | 结果 |
|---|---|---|
| 一、模型看见图片 | 贴入首图，询问图内四位数；Kimi 回复包含 `9682`。服务端不再报上传 404，时间线中 user input 有 1 张图。 | **完成**。 |
| 二、红线 11 | 紧接首图回答后再问图中数字；Kimi 回复仍包含 `9682`，说明历史图片可见。但 guard 精确为 `usage prompt=1894 completion=71 cached=1834 · drift=Clean · reconcile=BetterThanExpected{"predicted":1792,"actual":1834,"surplus":42} · window=Healthy{"turns":1,"hit_percent":96,"low_streak":0}`。要求的 `预测 == 实际` 未满足。 | **阻塞**：[089](089-kimi-image-cache-accounting.md)。 |
| 三、undo / redo | 连续 undo 到带图轮次后，页面时间线仍有 1 张用户图片（应为 0），故视觉撤销失败；redo 后下一轮询问，Kimi 仍回复 `9682`，证明 server history 已恢复，但不能代替时间线的消失/回来断言。 | **阻塞**：[090](090-image-undo-timeline.md)。 |
| 四、降级可见 | 尝试以不落盘的配置覆盖选择 DeepSeek，loader 未采用该特殊文件描述符，server 启动横幅仍为 Kimi；该调用不计作 DeepSeek 证据。静态调用链表明 HTTP 层会先向每个 provider 的 `upload_base_url` 上传，再到 adapter 生成 `ImagesDropped`，所以非视觉 provider 的降级可能在到达 adapter 前失败。 | **阻塞**：[091](091-nonvisual-image-ingress.md)。 |
| 五、不选图老路 | 同一真实会话发送 `只回复 TEXT-OK，不要解释。`，模型回复精确 `TEXT-OK`；guard 为 `usage prompt=1938 completion=41 cached=1834 · drift=Clean · reconcile=BetterThanExpected{"predicted":1792,"actual":1834,"surplus":42} · window=Healthy{"turns":3,"hit_percent":96,"low_streak":0}`。 | **完成**。 |
| 六、多张图 | 同时贴入数字 `9682`、`8799` 的两图并要求按出现顺序回答；回复同时包含二者，且 `9682` 出现在 `8799` 前。 | **完成**。 |

本 issue 不硬修以上三个新问题；M11 终点仍须在 089–091 完成后重新跑完整六条验收。

---

## 真机记录（091 续跑 · 2026-08-05）

091 已修复并作一次串行真实 DeepSeek 浏览器验证：随机图内数字为 **6636**，模型明确回复
`我看不到图片内容`，同一轮 UI 显示 `adjustments=ImagesDropped{\"count\":1}`。因此第四条不再阻塞。
090 的 Chromium 时间线断言已确认 undo 后图片卡片 `1 → 0`、redo `0 → 1`，以及文件名、缩略图和 object
URL 生命周期；结合上表中 Kimi redo 后仍能读出 `9682` 的真机证据，第三条不再阻塞。

仍只剩 [089](089-kimi-image-cache-accounting.md)：第二轮图片历史的缓存预测必须从
`predicted=1792` 对齐到实际 `1834`，随后才重跑第二条 Kimi 真机验收。

---

## 真机记录（最终六项 · 2026-08-05）

089 修复后，以同源静态 server、真实 Chrome 和默认 Kimi 前台串行完成五笔 Kimi 请求；没有并发的
付费调用。两张 29,878-byte 随机 PNG 中的数字依次为 **3570**、**4639**。第四条采用 091 修复后的
单笔前台真实 DeepSeek 浏览器证据（同一前端、随机图数字 **6636**）；未读取、输出、修改或提交
`providers.toml`、api_key 或图片本体。

| 验收 | 真机断言与观察 | 结果 |
|---|---|---|
| 一、模型真的看见了 | 首图后最后模型气泡精确为 `3570`。 | **完成** |
| 二、红线 11 | 第 2 轮最后模型气泡为 `3570`，guard 精确为 `usage prompt=1895 completion=72 cached=1834 · drift=Clean · reconcile=Match{"predicted":1834,"actual":1834} · window=Healthy{"turns":2,"hit_percent":98,"low_streak":0}`。 | **完成** |
| 三、undo / redo | 先撤纯文本轮仍为 1 张卡片，再撤带图轮，`.user-input` 精确为 `0`；两次 redo 后精确为 `1`，再问模型最后气泡为 `3570`。 | **完成** |
| 四、降级可见 | 091 的真实 DeepSeek 轮次：模型精确回复 `我看不到图片内容`，同轮 UI 精确显示 `adjustments=ImagesDropped{"count":1}`。 | **完成** |
| 五、不选图 = 老路不变 | 纯文本请求最后模型气泡精确为 `TEXT-OK`。 | **完成** |
| 六、多张图 | 同时贴两图后最后模型气泡精确为 `3570 4639`，与输入顺序相同。 | **完成** |
