// image-manager.js —— 图片管理面板的 DOM 绑定：选图 input → 存 → 列表 →
// 把链接消息填进输入框。纯 UI 胶水，不含 IndexedDB 逻辑（那是 image-store.js
// 的职责，见该文件模块文档）。
//
// issue 129 §3「发链接」由实现者定确切形态：这里选的是**填进输入框，让用户
// 自己发**，不是自动插入并发送——存图不该单方面消耗一轮对话，用户看到链接
// 文本之后自己决定要不要连着别的话一起发出去。
//
// 生命周期跟会话对齐：`refresh(sessionId)` 要在 index.html 的 openSession 成功
// 之后调一次（带上 `host.sessionId()`），把 `sessionId` 传 `null` 表示没有打开
// 的会话——这时面板整体禁用，不会有任何一次 IndexedDB 调用在 openSession 之前
// 发生（image-store.js 模块文档的约束 2）。

import { addImage, listImages, resolveImage, MAX_IMAGE_BYTES } from "./image-store.js";

/**
 * @param {{fileInput: HTMLInputElement, list: HTMLElement, status: HTMLElement, textInput: HTMLTextAreaElement}} elements
 * @returns {{refresh: (sessionId: string | null) => Promise<void>, resolveImage: (link: string) => Promise<{bytes: Uint8Array, mime: string}>, maxImageBytes: number}}
 */
export function mountImageManager({ fileInput, list, status, textInput }) {
  let sessionId = null;

  fileInput.addEventListener("change", async () => {
    const file = fileInput.files?.[0];
    fileInput.value = ""; // 允许连续选同一个文件
    if (!file) return;
    if (!sessionId) {
      status.textContent = "还没有打开的会话：先打开会话再选图";
      return;
    }
    status.textContent = `存入 ${file.name}（${file.size} 字节）…`;
    try {
      const { link } = await addImage(sessionId, file);
      appendLinkToInput(link);
      status.textContent = `已存入：${link}`;
      await renderList();
    } catch (err) {
      status.textContent = `存入失败：${err.message ?? err}`;
    }
  });

  function appendLinkToInput(link) {
    const note = `我上传了一张图：${link}`;
    textInput.value = textInput.value ? `${textInput.value}\n${note}` : note;
    textInput.focus();
  }

  async function renderList() {
    list.textContent = "";
    if (!sessionId) {
      list.textContent = "（没有打开的会话）";
      return;
    }
    let records;
    try {
      records = await listImages(sessionId);
    } catch (err) {
      list.textContent = `列表读取失败：${err.message ?? err}`;
      return;
    }
    if (records.length === 0) {
      list.textContent = "（还没有上传过图片）";
      return;
    }
    for (const record of records) {
      const row = document.createElement("div");
      row.textContent = `/uploads/${record.id}  ${record.mime}  ${record.bytes} 字节`;
      list.appendChild(row);
    }
  }

  async function refresh(nextSessionId) {
    sessionId = nextSessionId ?? null;
    fileInput.disabled = !sessionId;
    await renderList();
  }

  return {
    refresh,
    // 给真机验收 / 未来 130 的工具回调用：按当前会话解析链接，签名正是
    // issue 129 §4 要的 `resolveImage(link) -> Promise<{bytes, mime}>`。
    resolveImage: (link) => resolveImage(sessionId, link),
    maxImageBytes: MAX_IMAGE_BYTES,
  };
}
