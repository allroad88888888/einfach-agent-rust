// 唯一职责：把用户输入投影进时间线，并让这份本地投影随 undo/redo 回放。服务端没有
// 回显 `user_input` SSE 帧，因而图片卡片只能由浏览器保存原始 File 后重建；它不维护
// composer 草稿状态，也不参与 HTTP 请求。
import type { AgentId } from "@agent/protocol";

import { appendToTimeline, el } from "../dom";

const ROOT_AGENT: AgentId = "root";

interface UserInputEntry {
  text: string;
  images: readonly File[];
  card?: RenderedCard;
}

interface RenderedCard {
  remove(): void;
}

export interface UserInputTimeline {
  submit(text: string, images: readonly File[]): void;
  undo(): void;
  redo(): void;
}

/** 本地只保存用户提交的顺序；服务端的 `undo applied`/`redo applied` 各恰好反演一
 * 个输入轮次。纯文本仍占栈位，避免撤它时错误地撤掉更早的图片。 */
export function createUserInputTimeline(): UserInputTimeline {
  const applied: UserInputEntry[] = [];
  const redos: UserInputEntry[] = [];

  return {
    submit(text, images) {
      const entry: UserInputEntry = { text, images: [...images] };
      applied.push(entry);
      redos.length = 0;
      entry.card = renderUserInput(entry);
    },
    undo() {
      const entry = applied.pop();
      if (!entry) return;
      entry.card?.remove();
      entry.card = undefined;
      redos.push(entry);
    },
    redo() {
      const entry = redos.pop();
      if (!entry) return;
      entry.card = renderUserInput(entry);
      applied.push(entry);
    },
  };
}

function renderUserInput(entry: UserInputEntry): RenderedCard | undefined {
  const { text, images } = entry;
  // M11 前纯文本没有本地用户回显；保留该形状，但让它占 undo/redo 顺序栈位。
  if (images.length === 0) return undefined;
  const card = el("article", "user-input");
  if (text) card.append(el("p", "user-input-text", text));
  const imageView = renderImages(images);
  card.append(imageView.element);
  appendToTimeline(card, ROOT_AGENT);
  return {
    remove() {
      imageView.release();
      card.remove();
    },
  };
}

function renderImages(files: readonly File[]): { element: HTMLElement; release(): void } {
  const container = el("div", "user-images");
  const releases: Array<() => void> = [];
  for (const file of files) {
    const figure = el("figure", "user-image");
    const image = document.createElement("img");
    const objectUrl = URL.createObjectURL(file);
    let released = false;
    const release = (): void => {
      if (released) return;
      released = true;
      URL.revokeObjectURL(objectUrl);
    };
    image.addEventListener("load", release, { once: true });
    image.addEventListener("error", release, { once: true });
    image.src = objectUrl;
    image.alt = file.name || "用户发送的图片";
    figure.append(image, el("figcaption", undefined, file.name || "未命名图片"));
    container.append(figure);
    releases.push(release);
  }
  return {
    element: container,
    release() {
      for (const release of releases) release();
    },
  };
}
