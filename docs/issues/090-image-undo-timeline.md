# 090 图片卡片未随 undo / redo 回放

> ⚠️ **已废弃（superseded by s5）**：本文描述的 images 管线（`ContentBlock::Image` /
> `POST /files` / `upload_base_url` / `ImagesDropped` / 前端选图 / vision 子 agent 委托）
> 已被 s5 重构整体移除，现以 `POST /uploads` 上传端点 + `srv:vision/inspect` 工具取代。
> 正文仅作历史决策档案保留，不再反映当前实现。

**里程碑** M11 补充 · **依赖** [081](081-image-user-input.md) + [086](086-image-frontend.md) + [087](087-image-dogfood.md) · **模型** sonnet · **独测** 浏览器 · **状态** 完成

由 087 真机续跑发现：server history 的 undo/redo 已能让 Kimi 再次读出图片，但浏览器的用户图片卡片
没有跟随历史回放，造成时间线与真实状态不一致。

## 实测现象

带数字 `9682` 的图片轮次完成后，连续 undo 到该轮之前，页面中的用户图片计数仍为 `1`（预期 `0`）。
redo 后下一轮 Kimi 又能读出 `9682`，说明 history 已恢复；这不能证明旧图片卡片曾消失并按 redo 重建。

## 范围

使前端时间线在 `undo applied` 与 `redo applied` 后按 server history/revision 一致地撤去或重放用户图片
卡片；保持文件名、缩略图和 object URL 的释放规则正确。先找出 SSE 事件和当前 DOM 增量渲染的责任边界，
不要把整个时间线重写成另一套状态机。

## 验收（可判定）

1. 浏览器测试发送带图输入后执行 undo 到该轮之前：`.user-input` 中的图片卡片数精确从 `1` 变为 `0`。
2. 在同一会话 redo：图片卡片数从 `0` 变回 `1`，文件名与缩略图均存在；随后再发一条请求，server history
   中的图片引用仍会送入模型路径。
3. 纯文本轮次的 undo/redo 仍按原有顺序显示；重复 undo/redo 不产生重复图片卡片或泄漏未撤销的 object URL。
4. 对 081 的落盘/恢复和红线 11 不改变语义。

## 不在范围

- 不调整 Kimi 缓存预测（089）。
- 不处理非视觉 provider 的上传前短路（091）。
- 不以只更新「undo applied」提示文本取代时间线断言。

## 注意

- 先读 [IMAGES.md](../IMAGES.md) 的四条决定及 [INVARIANTS.md](../INVARIANTS.md) 第 1、5、11 条。
- `URL.revokeObjectURL` 是资源生命周期，不可为了 redo 简单删掉；需以可重建的浏览器状态实现正确回放。
- 实际踩坑：server 的 `undo applied` / `redo applied` 只有结果帧，不回显用户输入；浏览器须保留
  原始 `File` 作为本地时间线投影，纯文本也必须占一个栈位，不能由图片卡片数量推断轮次。

## 实做记录（完成 · 2026-08-05）

- `render/user_input.ts` 现在只维护用户输入的本地时间线：每次提交保存 `File`，undo 移除对应图片
  卡片并释放该卡片的 object URL，redo 用保存的 `File` 创建新的预览。纯文本没有新增卡片，但仍占
  栈位，故撤销它不会误撤更早的图片。
- `render/dispatch.ts` 只在实际收到 `undo applied` / `redo applied` 帧时反演这一投影，`main.ts`
  仍只是装配提交与渲染，未触碰 081 的落盘格式或红线 11。
- Chromium 直接加载 Vite 模块的断言：提交图片后 `.user-input` 为 `1`；撤销纯文本后仍为 `1`，再撤销
  图片为 `0`；redo 为 `1`，文件名 `nonce-9692.png` 和新缩略图 URL 均存在。重复 undo/redo 后仍恰有
  一张卡片；已撤销的前两个 URL 恰各 `revoke` 一次。`pnpm --filter @agent/web typecheck` 通过。

### 突变验证

将 `UserInputTimeline.undo` 的 `entry.card?.remove()` 故意替换为不移除卡片的空操作后，同一 Chromium
断言的原始红灯如下：

```text
node:internal/modules/run_main:107
    triggerUncaughtException(
    ^

page.evaluate: Error: 撤销图片轮次必须移除图片卡片
    at assert (eval at evaluate (:302:30), <anonymous>:7:68)
    at eval (eval at evaluate (:302:30), <anonymous>:17:5)
    at async <anonymous>:328:30
    at /Volumes/work/self/einfach-agent-rust/[eval1]:9:14

Node.js v24.14.0
```

恢复实现后，完整浏览器断言再次通过。
