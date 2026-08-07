# 086 前端选图 / 粘贴 / 拖拽

> ⚠️ **已废弃（superseded by s5）**：本文描述的 images 管线（`ContentBlock::Image` /
> `POST /files` / `upload_base_url` / `ImagesDropped` / 前端选图 / vision 子 agent 委托）
> 已被 s5 重构整体移除，现以 `POST /uploads` 上传端点 + `srv:vision/inspect` 工具取代。
> 正文仅作历史决策档案保留，不再反映当前实现。

**里程碑** M11 · **依赖** [085](085-http-image-ingress.md) · **模型** **haiku** · **独测** ✅ · **状态** 完成

**先读 [docs/IMAGES.md](../IMAGES.md)。** 前面几条做完之后，这条是接线 + UI。
**真机验收是 [087](087-image-dogfood.md)，不在本 issue。**

## 范围

`packages/web`：

1. **三条入口，`paste` 最重要**（截图直接贴是最常用的路径）：
   - `<input type="file" accept="image/*" multiple>`
   - `paste` 事件里的 `clipboardData.files`
   - 拖拽（`dragover` 要 `preventDefault`，否则浏览器会直接打开图片）
2. **发送前显示缩略图 + 文件名，能逐个删掉。**
   缩略图用 `URL.createObjectURL`，**卸载时 `revokeObjectURL`**（不 revoke 就是内存泄漏，
   而且不报错）。
3. `api.ts` 的 `sendInput` 带上图片。**不选图时请求体逐字节还是 `{"text":"..."}`。**
4. 时间线上把用户发的图**画出来**（`render/`）。
5. **前端先拦**：不是图片、或超过 100MB → 当场说明白，别把 400 留给服务端兜。
   官方推荐上限 4K（4096×2160），超了给个提示但不拦（那只是推荐值）。

## 验收（可判定）

1. `pnpm typecheck` 与 `pnpm build` 绿。
2. **不选图 = 老路不变**：纯文本发送的请求体与本 issue 之前**逐字节相同**。
3. **三条入口都能把图挂上**：选择、粘贴、拖拽各试一次，缩略图出现、文件名对。
4. **删得掉**：挂三张删中间一张，剩下两张顺序不变。
5. **非图片被拦**：拖一个 `.txt` 进去 → 有可读提示，且**没发出任何请求**。
6. **objectURL 被 revoke**：删除或发送后不再持有（能测就断言，测不了就在代码里
   写清楚 revoke 的时机）。

## 注意

- **`packages/protocol` 的类型是生成物，不手写镜像**（决策 2）。080 给
  `Adjustment` 加了变体，前端如果有穷举 `switch` 要补分支，**别用 `default` 吞掉**——
  吞掉了 083 的降级告警就到不了用户眼前，整条护栏白做。
- **红线 9（≤300 行）对前端同样有效**。「把本地图片交给 composer」是一件事，
  单独一个模块；`main.ts` 的定位是**纯接线**（文件开头写着），别往里堆。
- 收工验证前台跑完，含 `pnpm typecheck` 与 `cargo test --features ts`。
- 根 workspace 目前只定义了 `typecheck`，没有 `build` 脚本：`pnpm build` 会以
  `ERR_PNPM_RECURSIVE_EXEC_FIRST_FAIL Command "build" not found` 退出。前端实际构建命令是
  `pnpm --filter @agent/web build`；不要把根脚本缺失误判为附件构建失败，也不要在本 issue
  范围外顺手改 workspace 脚本。
- **本次实作确认**：附件发送后必须由 composer 统一回收 object URL；仅清空 `<input>` 不会回收
  预览 URL，也会使纯文本精确 body 的回归测试失去单一入口。

---

## 实做记录（完成 · 2026-08-04）

- `packages/web/src/composer/image_attachments.ts` 只负责本地附件状态：选择、粘贴、拖拽均经过
  image/100MiB 校验，按加入顺序保留，逐项删除，创建的 object URL 在删除、发送和销毁时 revoke；
  仅对超过 4096×2160 的图片给出非阻塞建议。渲染与接线分别留在 composer/render/main 的既有职责内。
- `sendInput` 有附件时发送二进制字节；无附件时保留既有精确 JSON 体。时间线在本地立即显示用户
  图片、文件名和缩略图，拒绝非图片时不发请求。
- `pnpm --filter @agent/web verify:images`：2 passed，0 failed。该 browserless 行为测试拦截
  fetch，逐字节断言纯文本 body 为 `{"text":"北京天气"}`，并断言图片的原始字节、名称与 MIME
  均被传入。`pnpm typecheck` 通过；`pnpm --filter @agent/web build` 通过（Vite 共 33 modules）。
