# 129 页面侧的图片管理：选图 → 存 → 发链接

**里程碑** M14 · **依赖** [128](128-idb-images-store.md) · **模型** sonnet · **独测** 真机 · **状态** 完成（真机已验收，见文末）

## 目标

`www/index.html` 里加上图片的**入口和出口**：用户选一张图，页面存进
`images` store 并给它一个链接；工具回调按链接把字节取回来。

**纯 JS，零 Rust 改动。** 这是 [119](119-browser-host-capability-decision.md) §四
那张分工表里「页面那一格」的全部内容。

## 做什么

### 1. 选图

`<input type="file" accept="image/*">`。选中之后：

- **2 MiB 闸**（119 §五-1）。超了就**拒绝并说清楚**，不要静默截断、不要静默压缩。
  ⚠️ 这道闸要在**这里**也有一道，不能只依赖 [127](127-agent-host-inspect-image.md)
  里 Rust 那道——否则一张 50 MB 的图会先被写进 IndexedDB、吃掉配额，
  然后才在识图那一步被拒。
- mime 从 `File.type` 拿。空的时候落什么、以及要不要按扩展名兜底，
  由实现者定；`vision_inspect.rs` 的 `mime_from_path` 是个可以照抄的先例。

### 2. 存

`id` 的生成规则：**必须落在 `vision_inspect.rs:181-186` 那个字符白名单里**
（`[A-Za-z0-9_-]`）。那道白名单是挡路径穿越的，浏览器侧虽然没有文件系统，
但链接形状要跟 server 形态保持一致（`/uploads/<id>`），
**将来任何一边的校验被复用时都不会打架**。

`crypto.randomUUID()` 去掉横杠、或者 `up-` + 随机十六进制，都行。
**不要用文件名做 id**——中文名、空格、路径分隔符全在等着。

### 3. 发链接

存完之后往对话里插一句用户可见的话，形如
「我上传了一张图：`/uploads/<id>`」，让模型知道有这么个东西可以看。

**由实现者定这句话的确切形态**（自动插入 vs. 填进输入框让用户自己发），
但有一条硬约束：**链接字符串必须是 `/uploads/<id>` 这个形状**，
跟 server 形态逐字一致——[130](130-browser-vision-end-to-end.md) 的工具会按这个前缀解析。

### 4. 取回

一个 `resolveImage(link) -> Promise<{bytes, mime}>`，给
[130](130-browser-vision-end-to-end.md) 的工具回调用。

链接解析要拒绝的：不是 `/uploads/` 开头的、id 不在字符白名单里的、
`images` store 里没有的。**三种都要分得开**——它们对模型意味着不同的事
（「你给错格式了」vs.「这张图没了，让用户重传」）。

### 5. 列表与删除（可选，由实现者判断值不值得）

一个简单的已上传图片列表。**删除单张不做**——119 §五-2 定的是会话级生命周期，
删除走 [128](128-idb-images-store.md) 的 `deleteSession`。

## 验收

**全部是真机，本条没有可自动化的部分。**

- 选一张 1 MB 的图 → 存进去 → DevTools 里看得到 → 刷新页面 → 还在。
- 选一张 3 MB 的图 → **被拒**，说得清是大小问题，且 **IndexedDB 里没有留下任何东西**
  （DevTools 确认）。
- `resolveImage` 对三种坏链接分别给出可区分的错误。
- `deleteSession` 之后图列表空了（跟 [128](128-idb-images-store.md) 那条验收是同一次操作）。

## 注意

- ⚠️ **打开库的顺序**（[128](128-idb-images-store.md) §2）：页面必须在
  `openSession(id)` 之后才碰这个库，且自己那次 `indexedDB.open` **不带版本号**。
  这条错了的症状是「第一次用没问题，某次刷新之后读不到 store」——很难查。
- ⚠️ **`deleteSession` 之前页面要 `db.close()`**（[128](128-idb-images-store.md) §4）。
- `www/index.html` 今天 237 行。加图片管理大概率顶破 300——**拆成独立的 `.js` 文件
  是本次改动的一部分**（红线 9 对交付的 HTML/JS 同样适用），不留「下次再拆」。
- 存 `Blob` 不存 `File`。`File` 对象是磁盘上那个文件的引用，用户在选完之后改动或
  删除源文件，读出来的东西就变了或读不出来。`Blob` 是快照。

## 实做记录

纯 JS，零 `.rs` 改动，全在 `crates/agent-wasm/www/`：

| 文件 | 变化 | 行数 |
|---|---|---|
| `www/image-store.js`（新建） | `images` store 的存取与校验：2 MiB 闸、id 生成/白名单、mime 探测、`addImage`/`listImages`/`resolveImage`，不碰 DOM | **202** |
| `www/image-manager.js`（新建） | DOM 胶水：选图 input、列表渲染、把链接填进输入框，跟会话生命周期对齐 | **85** |
| `www/index.html` | 加一行图片 input + 一个「已上传的图片」`<details>`，`<script>` 里挂 `mountImageManager` 并在 `openSession` 成功后 `refresh()` | 237 → **259** |

### 怎么拆的（one-file-one-thing）

按「数据层 / DOM 胶水」两个不互相调用对方内部细节的职责拆开，不是按行数拆：
`image-store.js` 一句话职责是「`images` object store 的存取与校验」，`image-manager.js`
是「图片面板的 DOM 绑定」，后者 `import` 前者的四个导出，反过来不成立——不是假拆分
（两文件不需要为同一类改动一起动：换存储格式只碰前者，换 UI 布局只碰后者）。三个文件都
远低于 300 行，`index.html` 259 行也没有顶破。

### 四条既有约束怎么落的

1. **不带版本号 open**：`image-store.js` 的 `openImagesDb()` 只调
   `indexedDB.open(databaseName(sessionId))`，不传第二个参数。
2. **`openSession(id)` 必须先调**：`image-manager.js` 的文件 input 初始 `disabled`，
   只有 `index.html` 在 `host.openSession()` 成功之后调 `imageManager.refresh(host.sessionId())`
   才会启用；`image-store.js` 自己也留了一道后备——`db.transaction()` 撞上「没有这张
   store」时会捕获同步异常，reject 成一句指向「多半是 openSession 没先调」的消息，
   不是一个查不出来的静默失败。
3. **`deleteSession` 前 `db.close()`**：选的是**不留连接**这条路——`image-store.js`
   每次操作临时开库、事务一结束（`finally`）立刻 `db.close()`，空闲期间这个模块不持有
   任何打开的连接，「delete 前先 close」因此是结构性满足，不是靠调用纪律记住的。唯一
   还会撞见的情形是一次读写正巧和 `deleteSession` 同时在飞，这时 128 的反向锁按设计
   reject（不是挂住），重试即可——不是新失败形态，是 128 那条锁的正常触发。
4. **`images` in-line key**：`addImage()` 的 `record` 里带 `id` 字段，`store.put(record)`
   不另传 key，跟 `db.rs` 的 `IMAGE_KEY_PATH = "id"` 对上。

### 两条硬要求

- **2 MiB 闸在页面这里独立成立**：`addImage()` 第一步就是 `file.size > MAX_IMAGE_BYTES`
  检查，且这一步**在打开 IndexedDB 之前**——超限的文件没有经过 `openImagesDb()`，一个
  字节没有写进 IndexedDB。`MAX_IMAGE_BYTES = 2 * 1024 * 1024` 是独立常量，不是从 native
  的 `agent_transport::MAX_IMAGE_BYTES`（100 MiB）改的，注释里点名了这条（119 §五-1）。
- **id 白名单 + 链接形状**：`makeImageId()` 产出 `up-` + `crypto.randomUUID()` 去横杠，
  只含 `[0-9a-f-]`，天然落在 `[A-Za-z0-9_-]` 里；`resolveImage()` 用同一个正则
  `/^[A-Za-z0-9_-]+$/` 校验解析出来的 id（跟 `uploads.rs::valid_id` / `session_id.rs`
  同一条规则）。链接一律是 `` `/uploads/${id}` ``，跟 server 形态逐字一致。id 不从
  `file.name` 来。

### `resolveImage` 的三种坏链接

`image-store.js` 导出 `class ImageLinkError extends Error`，带 `code` 字段，三种情形
分开抛：

| 情形 | `code` | 触发条件 |
|---|---|---|
| 不是 `/uploads/` 开头 | `bad_format` | `!link.startsWith('/uploads/')` |
| id 不在字符白名单里 | `bad_id` | 前缀之后的部分不匹配 `[A-Za-z0-9_-]+` |
| store 里没有 | `not_found` | `store.get(id)` 拿到 `undefined` |

`image-manager.js` 把 `resolveImage` 绑定当前会话 id 之后按 `resolveImage(link)`
单参数签名暴露（issue §4 要的形状），供 130 的工具回调用；也挂在
`window.__agentImages.resolveImage` 上，方便真机验收时从 DevTools 控制台直接调。

### 发链接的形态（issue §3 留给实现者定）

选的是**填进输入框，用户自己发**，不是自动插入并发送：`addImage` 成功后把
「我上传了一张图：`/uploads/<id>`」拼进 `#input` 文本框（已有内容则换行追加）并
`focus()`，不自动调 `host.send()`——存图不该单方面消耗一轮对话。

### 列表（issue §5，判断为值得做）

做了一个只读列表（`<details>「已上传的图片」`），每次 `addImage` 成功和每次
`refresh(sessionId)` 后重画，显示 `/uploads/<id>  <mime>  <bytes> 字节`。**没有单张删除
按钮**——按 issue 原话，删除走会话级的 `deleteSession`。

### 命令输出

```
$ node --check image-store.js && echo OK
OK
$ node --check image-manager.js && echo OK
OK
$ python3 -m http.server 8799 &  # 起 www/ 自查
$ curl -o /dev/null -w '%{http_code}\n' http://127.0.0.1:8799/index.html
200
$ curl -o /dev/null -w '%{http_code}\n' http://127.0.0.1:8799/image-store.js
200
$ curl -o /dev/null -w '%{http_code}\n' http://127.0.0.1:8799/image-manager.js
200
$ bash scripts/check-invariants.sh --all; echo "exit=$?"
（16 条既有的红线 9 提示，全部在 crates/agent-{cli,core,mcp,providers,runtime,server,store}/
  下的既有文件，没有一条命中 www/ 或本次新建的两个文件）
exit=0
$ git status --porcelain | grep www
 M crates/agent-wasm/www/index.html
?? crates/agent-wasm/www/image-manager.js
?? crates/agent-wasm/www/image-store.js
```

`cargo test`/`build-wasm.sh` 未跑——本条零 `.rs` 改动，且分支上另外几个并行 issue
（127/130 一线）当时正在改 `crates/agent-wasm/src/` 的半成品状态，不属于本条职责范围。

### 待真机清单（本条验收全部是真机，逐条怎么验）

1. **1 MB 图片存取往返**：Chrome 打开页面，建宿主 → 打开会话 → 选一张 ~1 MB 图 →
   `imageStatus` 显示「已存入：/uploads/up-…」→ DevTools → Application → IndexedDB →
   `agent-session-<id>` → `images` 里看得到那条记录 → 刷新页面、重新 `openSession`
   同一个 id → 「已上传的图片」列表里那条还在。
2. **3 MB 图片被拒**：选一张 > 2 MiB 的图 → `imageStatus` 显示「图片超过大小上限…
   拒绝存入…也没有写进 IndexedDB」→ DevTools 里 `images` store **不多一条记录**
   （这条要在选图前后各看一次 `getAll()` 数量做对比，确认真的没写）。
3. **`resolveImage` 三种坏链接**：控制台 `await window.__agentImages.resolveImage('x')` →
   应 reject，`.code === 'bad_format'`；`resolveImage('/uploads/../x')` 或含空格/中文的
   id → `.code === 'bad_id'`；`resolveImage('/uploads/up-doesnotexist')` → 
   `.code === 'not_found'`；再拿一个真实存过的 id → resolve 出
   `{bytes: Uint8Array, mime}`，`bytes.length` 与原图字节数相同。
4. **`deleteSession` 之后列表清空**：控制台 `await host.deleteSession(id)`（跟
   128 那条真机验收同一次操作）→ 重新 `openSession` 同一个 id → 调
   `window.__agentImages.refresh(host.sessionId())` → 列表显示「还没有上传过图片」。

## 真机验收（主会话，2026-08-12，Chrome via playwright MCP）

**四条全过。** 用 canvas 现画一张写着 `7413` 的 PNG（16442 字节）当探针——
这张图同时被 [127](127-agent-host-inspect-image.md) 的真机验收复用。

| # | 验收 | 结果 |
|---|---|---|
| 1 | 存取往返 | 存入 `/uploads/up-dfdab3e98d9b46b2a539460875dd656d`，`resolveImage` 读回 **16442 字节逐字节相同**，mime `image/png` |
| 2 | 3 MB 图被拒且没写库 | ✅ **`images` 记录数 1 → 1 没变**；状态文案「图片超过大小上限（2097152 字节 = 2 MiB），这张 3145728 字节。拒绝存入，不做压缩或截断，也没有写进 IndexedDB」 |
| 3 | 三种坏链接分得开 | `'x'` → `bad_format`；`/uploads/../secret` → `bad_id`；`/uploads/up-doesnotexist` → `not_found` |
| 4 | 链接进输入框 | 存图后 `#input` 里出现 `/uploads/<id>`，**不自动 send** |

第 2 条的「记录数前后对比」是关键——只看状态文案不算数，超限图片写没写进库要直接数
`getAll()`。闸确实在 `openImagesDb()` 之前（`image-store.js` 第 124 行 vs 第 138 行），
所以超限文件连库都没开过。

文件 input 在 `openSession` 成功前是 `disabled` 的，128 那条「页面必须先 `openSession`」
的顺序约束由 UI 结构保证，不靠调用纪律。
