// 唯一职责：管理 composer 里本地图片草稿的入口、校验、缩略图与 object URL 生命周期。
// 发往 HTTP 的编码由 `api.ts` 完成，已经发出的图片由 `render/user_input.ts` 负责画到
// 时间线；本模块只保有「尚未发送」的 File。
const MAX_IMAGE_BYTES = 100 * 1024 * 1024;
const RECOMMENDED_WIDTH = 4096;
const RECOMMENDED_HEIGHT = 2160;

interface Attachment {
  file: File;
  objectUrl: string;
  row: HTMLElement;
}

export interface ImageAttachmentElements {
  form: HTMLFormElement;
  fileInput: HTMLInputElement;
  tray: HTMLElement;
  message: HTMLElement;
}

export interface ImageAttachments {
  takeFiles(): File[];
}

export function createImageAttachments(elements: ImageAttachmentElements): ImageAttachments {
  const attachments: Attachment[] = [];
  const { fileInput, form, message, tray } = elements;

  const addSelectedFiles = (files: Iterable<File>): void => {
    let rejected = false;
    for (const file of files) {
      if (!file.type.startsWith("image/")) {
        rejected = true;
        continue;
      }
      if (file.size > MAX_IMAGE_BYTES) {
        rejected = true;
        continue;
      }
      addAttachment(file);
    }
    message.textContent = rejected ? "只支持不超过 100MB 的图片附件；未加入的文件没有发送。" : "";
  };

  const addAttachment = (file: File): void => {
    const objectUrl = URL.createObjectURL(file);
    const row = document.createElement("div");
    row.className = "image-attachment";

    const image = document.createElement("img");
    image.src = objectUrl;
    image.alt = file.name || "待发送图片";

    const details = document.createElement("div");
    const name = document.createElement("div");
    name.className = "image-attachment-name";
    name.textContent = file.name || "未命名图片";
    name.title = name.textContent;
    details.append(name);
    void addDimensionWarning(file, details);

    const remove = document.createElement("button");
    remove.type = "button";
    remove.textContent = "删除";
    remove.ariaLabel = `删除 ${file.name || "图片"}`;

    const attachment: Attachment = { file, objectUrl, row };
    remove.addEventListener("click", () => removeAttachment(attachment));
    row.append(image, details, remove);
    attachments.push(attachment);
    tray.append(row);
  };

  const removeAttachment = (attachment: Attachment): void => {
    const index = attachments.indexOf(attachment);
    if (index < 0) return;
    attachments.splice(index, 1);
    URL.revokeObjectURL(attachment.objectUrl);
    attachment.row.remove();
  };

  fileInput.addEventListener("change", () => {
    addSelectedFiles(Array.from(fileInput.files ?? []));
    // 清空后，用户删掉同一张又重新选它，仍然会触发 change。
    fileInput.value = "";
  });

  form.addEventListener("paste", (event) => {
    const files = Array.from(event.clipboardData?.files ?? []);
    if (files.length === 0) return;
    event.preventDefault();
    addSelectedFiles(files);
  });
  form.addEventListener("dragover", (event) => {
    event.preventDefault();
    form.classList.add("is-dragging");
  });
  form.addEventListener("dragleave", () => form.classList.remove("is-dragging"));
  form.addEventListener("drop", (event) => {
    event.preventDefault();
    form.classList.remove("is-dragging");
    addSelectedFiles(Array.from(event.dataTransfer?.files ?? []));
  });

  // 单页目前没有卸载 composer 的路由，但离开页面仍是草稿生命周期的终点。
  window.addEventListener("pagehide", () => releaseAttachments(attachments));

  return {
    takeFiles(): File[] {
      const files = attachments.map((attachment) => attachment.file);
      releaseAttachments(attachments);
      return files;
    },
  };
}

function releaseAttachments(attachments: Attachment[]): void {
  for (const attachment of attachments) {
    URL.revokeObjectURL(attachment.objectUrl);
    attachment.row.remove();
  }
  attachments.length = 0;
}

async function addDimensionWarning(file: File, details: HTMLElement): Promise<void> {
  const image = new Image();
  const objectUrl = URL.createObjectURL(file);
  try {
    await new Promise<void>((resolve, reject) => {
      image.onload = () => resolve();
      image.onerror = () => reject(new Error("图片尺寸读取失败"));
      image.src = objectUrl;
    });
    if (image.naturalWidth <= RECOMMENDED_WIDTH && image.naturalHeight <= RECOMMENDED_HEIGHT) return;
    const warning = document.createElement("div");
    warning.className = "image-size-warning";
    warning.textContent = `${image.naturalWidth}×${image.naturalHeight}，建议不超过 4096×2160（仍会发送）`;
    details.append(warning);
  } catch {
    // 格式虽声明为 image/*，浏览器仍可能无法解码；交给服务端上传路径给出明确错误。
  } finally {
    URL.revokeObjectURL(objectUrl);
  }
}
