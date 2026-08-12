// image-store.js —— `images` object store 的存取与校验，不碰 DOM。
//
// issue 129（依赖 128）。schema 由 Rust 建（crates/agent-wasm/src/db.rs 的
// `create_missing_stores`），这个文件是「谁读写它」那一半——128 的分工表写死了
// images 的读写全归页面，Rust 一个字节不碰。
//
// 三条从 db.rs 模块文档搬过来、页面必须遵守的顺序约束（违反不当场报错，症状是
// 「第一次好用，某次刷新之后读不到 store」）：
//
//   1. 这里的 `indexedDB.open(name)` 不带版本号——版本号整个归 Rust 一方。
//   2. 必须在 `AgentHost.openSession(id)` 成功之后才调用本模块——否则会先建出
//      一个版本 1、没有任何 store 的空库，Rust 随后升级它时页面这次读会拿到
//      「没有 images 这张 store」的错误。本文件不替调用方检查这一条（它不知道
//      有没有 openSession 过），调用方对齐见 image-manager.js 的 refresh()。
//   3. 调 `deleteSession` 之前页面要 db.close()。**这里选的做法是干脆不留连接**
//      ——每次操作临时开、事务一结束（成功或失败）立刻关，空闲期间这个模块
//      不持有任何打开的连接。所以「delete 之前先 close」这条约束在这里是结构性
//      满足的，不是靠调用纪律记住的；唯一还会撞见的情形是一次读写正巧和
//      deleteSession 同时在飞，那时 128 的反向锁会按设计 reject（不是挂住），
//      重试即可。
//
// 2 MiB 闸（119 §五-1）在这里也有一道，且在**任何 IndexedDB 写入之前**用
// `file.size` 挡掉——不读字节、不摸库，超限的图片不会吃掉一个字节的配额。
// 这个数字和 native 的 100 MiB（agent-transport::MAX_IMAGE_BYTES）不是同一个
// 常量，理由见 119 §五-1：IndexedDB 配额是整个 origin 共享的。
//
// 存的是 Blob，不是 File：File 是磁盘上那个文件的引用，用户选完之后改动或删除
// 源文件，读出来的东西会变或读不出来；Blob 是快照。`File.slice()` 返回的就是
// 一个不带文件名/mtime 的纯 Blob。

/** 单张图片上限：2 MiB（119 §五-1，不是 native 的 100 MiB）。 */
export const MAX_IMAGE_BYTES = 2 * 1024 * 1024;

/** 跟 db.rs 的 `IMAGE_STORE` 常量同一个名字。 */
const IMAGE_STORE = "images";

/** 链接形状的前缀，跟 server 形态（agent-server/src/http/uploads.rs）逐字一致。 */
const LINK_PREFIX = "/uploads/";

/** id 字符白名单，跟 `vision_inspect.rs` 的链接解析 / `uploads.rs::valid_id` /
 * `agent-wasm/src/session_id.rs` 同一条规则：`[A-Za-z0-9_-]`，非空。 */
const ID_PATTERN = /^[A-Za-z0-9_-]+$/;

const EXTENSION_MIME = {
  png: "image/png",
  jpg: "image/jpeg",
  jpeg: "image/jpeg",
  webp: "image/webp",
  gif: "image/gif",
};

/** 三种坏链接分开报——模型看到的是 `code`，不是拼在消息里靠猜。 */
export class ImageLinkError extends Error {
  constructor(code, message) {
    super(message);
    this.name = "ImageLinkError";
    this.code = code; // 'bad_format' | 'bad_id' | 'not_found'
  }
}

function databaseName(sessionId) {
  return `agent-session-${sessionId}`;
}

/** mime 优先取 `File.type`；空的时候按扩展名兜底，四种规则照抄
 * `vision_source.rs::mime_from_path`；都不认识就是 `application/octet-stream`。 */
function mimeFromFile(file) {
  if (file.type) return file.type;
  const ext = file.name.split(".").pop()?.toLowerCase();
  return (ext && EXTENSION_MIME[ext]) || "application/octet-stream";
}

function makeImageId() {
  return `up-${crypto.randomUUID().replace(/-/g, "")}`;
}

function openImagesDb(sessionId) {
  return new Promise((resolve, reject) => {
    if (!sessionId) {
      reject(new Error("还没有打开的会话：先调 openSession(id)"));
      return;
    }
    // 不带版本号：按当前版本打开，不触发升级（约束 1）。
    const request = indexedDB.open(databaseName(sessionId));
    request.onsuccess = () => resolve(request.result);
    request.onerror = () =>
      reject(request.error ?? new Error("打开图片库失败"));
    request.onblocked = () =>
      reject(new Error("打开图片库被阻塞：有别的标签页正在升级这个会话的库"));
  });
}

function missingStoreError(err) {
  return new Error(
    `打不到 images 这张 store（${err?.message ?? err}）——多半是 openSession(id) ` +
      "没有在这之前调用过（约束 2，见本文件模块文档）。",
  );
}

/** 把一次 `store.xxx()` 调用包成 Promise：事务成功才 resolve，失败/中止都 reject。 */
function runTransaction(db, mode, work) {
  return new Promise((resolve, reject) => {
    let tx;
    try {
      tx = db.transaction(IMAGE_STORE, mode);
    } catch (err) {
      reject(missingStoreError(err));
      return;
    }
    const store = tx.objectStore(IMAGE_STORE);
    const request = work(store);
    tx.oncomplete = () => resolve(request?.result);
    tx.onerror = () => reject(tx.error ?? new Error("images store 事务失败"));
    tx.onabort = () => reject(tx.error ?? new Error("images store 事务中止"));
  });
}

/**
 * 存一张图片：2 MiB 闸 → 生成 id → 转成 Blob → `put`。
 * 超限时 reject，且这次调用**没有碰过 IndexedDB**——不会有任何字节写进去。
 * @returns {Promise<{id: string, link: string, mime: string, bytes: number}>}
 */
export async function addImage(sessionId, file) {
  if (file.size > MAX_IMAGE_BYTES) {
    throw new Error(
      `图片超过大小上限（${MAX_IMAGE_BYTES} 字节 = 2 MiB），这张 ${file.size} 字节。` +
        "拒绝存入，不做压缩或截断，也没有写进 IndexedDB。",
    );
  }
  if (file.size === 0) {
    throw new Error("图片内容为空");
  }
  const mime = mimeFromFile(file);
  const blob = file.slice(0, file.size, mime); // Blob 快照，不留 File 引用
  const id = makeImageId();
  const record = { id, blob, mime, bytes: file.size, addedAt: Date.now() };

  const db = await openImagesDb(sessionId);
  try {
    await runTransaction(db, "readwrite", (store) => store.put(record));
  } finally {
    db.close();
  }
  return { id, link: `${LINK_PREFIX}${id}`, mime, bytes: file.size };
}

/**
 * 列出这个会话存过的图片（不含 blob 本身，只给列表渲染用的元数据）。
 * @returns {Promise<Array<{id: string, mime: string, bytes: number, addedAt: number}>>}
 */
export async function listImages(sessionId) {
  const db = await openImagesDb(sessionId);
  try {
    const records = await runTransaction(db, "readonly", (store) =>
      store.getAll(),
    );
    return (records ?? [])
      .map(({ id, mime, bytes, addedAt }) => ({ id, mime, bytes, addedAt }))
      .sort((a, b) => a.addedAt - b.addedAt);
  } finally {
    db.close();
  }
}

/**
 * 按链接取字节。三种坏链接分开报（`ImageLinkError.code`）：
 * `bad_format`（不是 `/uploads/` 开头）、`bad_id`（id 不在字符白名单里）、
 * `not_found`（store 里没有）——它们对模型意味着不同的事：前两种是「你给错
 * 格式了」，最后一种是「这张图没了，让用户重传」。
 * @returns {Promise<{bytes: Uint8Array, mime: string}>}
 */
export async function resolveImage(sessionId, link) {
  if (typeof link !== "string" || !link.startsWith(LINK_PREFIX)) {
    throw new ImageLinkError(
      "bad_format",
      `图片链接必须以 ${LINK_PREFIX} 开头，收到：${link}`,
    );
  }
  const id = link.slice(LINK_PREFIX.length);
  if (!ID_PATTERN.test(id)) {
    throw new ImageLinkError(
      "bad_id",
      `图片链接里的 id 不合法（只允许 [A-Za-z0-9_-]）：${id}`,
    );
  }

  const db = await openImagesDb(sessionId);
  let record;
  try {
    record = await runTransaction(db, "readonly", (store) => store.get(id));
  } finally {
    db.close();
  }
  if (!record) {
    throw new ImageLinkError(
      "not_found",
      `这张图片已经不在了（会话里没有 id=${id} 的记录），让用户重传：${link}`,
    );
  }
  const bytes = new Uint8Array(await record.blob.arrayBuffer());
  return { bytes, mime: record.mime };
}
