# 128 IndexedDB 加 `images` store + `deleteSession`

**里程碑** M14 · **依赖** [119](119-browser-host-capability-decision.md) · **模型** opus · **独测** 真机（兼容性风险） · **状态** 完成（真机已验收，见文末）

## 目标

给会话库加一张放图片字节的 object store，并新造一条**今天完全不存在**的路径：
删掉一个会话。

**无 Rust 逻辑依赖，第一天就能开工**，但它是 M14 里唯一会碰**已有会话数据**的一条，
所以派 opus + 真机验收。

## 为什么这条有兼容性风险

`agent-wasm/src/db.rs:35` 今天是 `factory.open_with_u32(&format!("agent-session-{id}"), 1)`。
**已经存在的会话库都在版本 1 上**，其中包括 M13 真机验收留下的那些。
版本升到 2，它们下次打开会走 `onupgradeneeded`。

好消息：`create_store_if_missing`（`db.rs:59`）已经写对了——它只建缺的那张，
不碰已有的 `journal`。所以升级本身是安全的。但**「安全」需要被证明**，
而不是被假设：M13 那条「刷新 4 次后同一会话从 IndexedDB 重放 12 条消息」的验收要重跑。

## 做什么

### 1. 版本 1 → 2，加 `images` store

`create_store_if_missing` 改成建两张（`journal` + `images`），
或者改成一个「按名字列表建缺的」的循环。**保持它今天那个「拿不到就静默返回」的
错误处理**——`db.rs:56-58` 给了理由，别改成 panic。

### 2. schema：谁拥有它，谁读写它

⚠️ **这是本条最容易做错的地方**，因为它跨了 Rust/JS 的分工线
（[119](119-browser-host-capability-decision.md) §四）：

| | 谁 |
|---|---|
| **建** `images` store（只能在 `onupgradeneeded` 里建，而 `open` 在 Rust 手上） | **Rust** |
| **读写** `images` 里的数据 | **页面** |

页面拿数据的方式：自己 `indexedDB.open('agent-session-<id>')`
——**不带版本号**。不带版本号 = 「按当前版本打开，不触发升级」，
于是版本号这件事完全由 Rust 一方拥有，两边不会打架。

**由此得到一条对页面的硬约束**：`openSession(id)` 必须在页面碰这个库**之前**
调用过。否则页面会把一个空库建在版本 1 上、没有任何 store，
随后 Rust 的 `open(…, 2)` 会去升它——能work，但页面那次读会拿到
「没有这张 store」的错误。**这条要写进给页面的文档**，并在 `www/index.html`
的示例里体现出正确顺序。

### 3. 值的形状：直接存二进制，不要走现有那个 KV

`persist/idb/web_kv.rs` 的模块文档自己写死了：

> key/value 都当 UTF-8 字符串存，不当二进制 …… **如果 `KvStore` 未来有别的调用方
> 需要塞真正的二进制 value，这个假设要重新评估**

图片正是那个调用方，而**正确答案是不去评估它**——`images` store 跟 journal
是两张互不相干的 store，图片直接存 `Blob`（IndexedDB 的结构化克隆原生支持），
`KvStore`/`IdbDatabaseKv` **一个字节都不动**。

记录形状建议 `{ id, blob, mime, bytes, addedAt }`，但由实现者定；
**唯一的硬要求**是不要 base64（+33% 体积，还把二进制混进字符串编码）。

### 4. `AgentHost.deleteSession(id)`

```rust
/// 删掉一个会话：journal 与图片一起没。
///
/// 删的是**整个库**，所以图片不需要单独清——这正是 119 §五-3 选「同一个库」
/// 换来的东西。
#[wasm_bindgen(js_name = deleteSession)]
pub fn delete_session(&self, id: String) -> js_sys::Promise;
```

⚠️ **`deleteDatabase` 在有连接开着时会阻塞**（`onblocked`，不是报错，是**挂住**）。
所以必须先关连接。两个问题必须回答：

1. 删的是**当前打开的**会话时怎么办——关掉 `live`？拒绝？由实现者定并写明理由。
2. 页面自己那条连接（§2）也开着。**这条约束要写进给页面的文档**：
   调 `deleteSession` 之前页面要先 `db.close()`。
3. `onblocked` 触发时**必须 reject 而不是永远 pending**，否则页面上那个按钮
   会永远转圈且没有任何错误信息。

## 验收

- **真机（兼容性主证据）**：拿一个**在本条落地之前**建好的会话库（版本 1、
  有 journal 数据），升级后打开 → **journal 一条不丢，重放出来的历史逐字节相同**。
  这是 M13 那条「刷新 4 次重放 12 条消息」的重跑。
  ⚠️ 别用一个新建的库来验这条——新库直接建在版本 2 上，走的不是升级路径，
  **验不到任何东西**。
- **真机**：新建会话 → 两张 store 都在（DevTools → Application → IndexedDB 肉眼可见）。
- **真机**：页面往 `images` 里存一个 Blob，刷新页面后读得回来，字节相同。
- **真机**：`deleteSession(id)` 之后，DevTools 里那个库消失；同 id 再
  `openSession` 得到一个**空会话**（不是报错，不是旧历史）。
- **真机反向锁**：页面连接开着时调 `deleteSession` → **reject 并说明原因**，
  不是挂住。

## 注意

- 红线 9：`db.rs` 今天 107 行，加一张 store 不会顶破；但 `deleteSession` 的
  连接管理（关 `live`、等 `onblocked`）有可能。到 300 行就拆，别硬塞。
- **不要给 `KvStore` 加 `delete` 或 `delete_range`。** `persist/idb/mod.rs`
  那段「journal 只增不删」的设计说明里点名了这是「已知的、刻意留到之后的存储空间
  问题」——本条是**另一张 store 的另一件事**，不要顺手把那个也办了。
- 本条**不做图片的选取、上限校验、UI**。那是 [129](129-page-image-manager.md)。
  本条只保证「有地方放，且能一起删」。

## 实做记录

改了四个文件，**全在 `crates/agent-wasm/`**（并行的 124/126 等在同一分支上动别的文件，
零重叠）：

| 文件 | 变化 | 行数 |
|---|---|---|
| `src/db.rs` | 版本 1→2、两张 store、`delete()`、`onblocked`、`close_on_versionchange` | 107 → **245** |
| `src/host.rs` | `deleteSession` 一个方法 + 模块文档「五件事」→「六件事」 | 194 → 232 |
| `src/lib.rs` | 模块表里 `db` 那一行的描述 | 54（不变） |
| `Cargo.toml` | web-sys 加 `IdbObjectStoreParameters` | +3 |

`db.rs` 245 行**没拆**：红线 9 的上限没顶破，而且「开」和「删」共用库名、版本号、
`onblocked` 桥接三样知识，给页面的那份契约（开不带版本号 / 先 `openSession` / 删前
`db.close()`）也是**一份**文档——拆开只会让它跨两个文件。真要再长（比如加 `images`
的配额清理）就按「schema+开」/「删」切。

### 三个坑各自怎么决的

1. **schema 跨分工线**：`images` 由 `create_missing_stores`（Rust，`onupgradeneeded`）建，
   `keyPath: "id"` 走 in-line key ——页面 `put(record)` 不用另传 key，记录里除 `id`
   以外的字段 Rust 一概不认识。**对页面的两条顺序约束写进了 `db.rs` 模块文档**
   （不带版本号 open、`openSession` 必须先调），`host.rs` 的 `deleteSession` 文档转引。
   `www/index.html` 的示例**没动**——那是 129 的地盘，本条只落文档契约。
2. **`deleteDatabase` 的连接管理**：三个问题的答案分别是——
   - **删当前会话**：允许，`delete_session` 先 `live.take()` 把它整个放掉（`sessionId()`
     变 `undefined`）。选「关掉」而不是「拒绝」的理由：这个宿主没有别的关会话的入口，
     拒绝等于「正在看的那个会话永远删不掉」。
   - **Rust 那条连接**：光 drop 不够——drop 只放掉 JS 侧引用，连接要等 GC 回收对象才关，
     GC 什么时候来没保证。所以 `open()` 出来的每条连接都挂 `onversionchange → close()`
     （IndexedDB 的标准自保）。这条同时救了「另一个标签页开着同一个会话」。
   - **`onblocked`**：`await_open_request` 三个回调都接，blocked 一律 **reject**。
     顺手把 `open` 也接上了——版本号一升，`open` **第一次**变得可能被别的标签页挡住，
     不接就是一个永不 settle 的 Promise（跟删除按钮永远转圈同一个病）。
   - 额外：`deleteSession` 撞上在飞的一轮用 `try_borrow_mut` **reject 而不是 panic**
     （破坏性操作，按钮随时可能被按到）。
3. **Blob 不 base64**：`images` 与 `journal` 是两张互不相干的 store，`KvStore` /
   `IdbDatabaseKv` **一个字节没动**，`web_kv.rs` 那句「假设要重新评估」按 issue 说的
   不去评估。

### 一条 reject 的准确含义（写进了 `db::delete` 的文档）

被 `onblocked` 挡住的删除请求**在浏览器里仍然挂着**，等最后一条连接关掉照样生效。
所以那句错误是「现在没删成，去关掉别的连接再来」，不是「什么都没发生」。

### 验收：编译期过了，五条真机全部**待验**

- `bash scripts/build-wasm.sh` **绿**（wasm-bindgen 产物里 `deleteSession(id: string):
  Promise<any>` 在 `agent_wasm.d.ts` 第 35 行）。
- `cargo test --workspace` 绿（1850+ 全过）——**在一份 `git archive HEAD` + 只叠我这
  四个文件的干净副本上跑的**。主工作树当时被并行 issue 的半成品（`agent-tools` 的
  `vision_*`）挡着编译，那跟本条无关：`agent-wasm` 根本不是主 workspace 的 member
  （根 `Cargo.toml` 的 members 里没有它），结构上就影响不到。
- `bash scripts/check-invariants.sh --all` 退出码 0，`agent-wasm` 零命中。

**真机清单（Chrome + 真 key，逐条怎么验）**：

1. **兼容性主证据（升级路径）**——⚠️ 必须拿**本条落地之前**建好的库：先 `git stash`
   本条改动（或直接用 M13 那次 dogfood 留下的 profile）跑旧版页面，发若干条消息，
   DevTools → Application → IndexedDB 确认 `agent-session-<id>` 是 **version 1**；
   再切回本条的构建刷新 → 同一个 id `openSession` → 重放出来的历史与升级前**逐字节
   相同**（M13 那条「刷新 4 次重放 12 条消息」原样重跑）。
   **新建一个库验不到任何东西**——新库直接建在版本 2 上，走的不是 `onupgradeneeded`
   的升级分支。
2. **两张 store**：新建会话 → DevTools 里 `journal` 与 `images` 都在，`images` 的
   Key path 显示 `id`。
3. **Blob 往返**：控制台里 `indexedDB.open('agent-session-<id>')`（**不带版本号**）→
   `put({id:'x', blob, mime, bytes, addedAt})` → 刷新页面 → `get('x')` 读回来，
   `blob.size` 与前后 `arrayBuffer()` 的字节相同。
4. **删**：`await host.deleteSession(id)` → DevTools 里那个库消失；同 id 再
   `openSession` 拿到**空历史**（不是报错，不是旧历史）。删的是当前会话时，
   `host.sessionId()` 变 `undefined`。
5. **反向锁**：页面先 `indexedDB.open(...)` 拿一条连接**不 close**，再调
   `deleteSession` → Promise **reject**，消息是「删不掉：还有别的连接开着……」，
   不是挂住。随后把那条连接 `db.close()`，DevTools 里库应当**随即消失**（验证上面
   那条「reject ≠ 没删」）。
6. **顺便**：`onversionchange` 自保生效——开两个标签页同一个会话，在 A 里
   `deleteSession`，B 那条 Rust 连接应当自己关掉、A 不被 blocked。

## 真机验收（主会话，2026-08-11，Chrome via playwright MCP + 真 Kimi key）

**六条全过。** provider = kimi / kimi-k3，key 只以长度示人（`key=51 字符`，111 契约第 4 条）。

### 怎么造出「版本 1 的库」的（这一步是整条验收的关键）

issue 反复强调「新建库验不到任何东西」。做法是 **`git worktree add --detach <path> HEAD`**
——本分支上 M14 的改动一行都没提交，所以 `HEAD` 就是 M14 之前的代码。在那份 worktree 里
`build-wasm.sh`，用 **同一个端口 127.0.0.1:8787** 起服务（IndexedDB 按 origin 隔离，
换端口就换了库，测试当场作废），跑出一个真的版本 1 的会话，再把服务换回 M14 构建。

老构建的旁证：工具表只有 `web:page/title` + `web:page/url` 两条，没有 124 加的
`web:source/echo`。

⚠️ **踩过一次坑**：换完服务第一次打开，工具表仍是 2 条、库仍是版本 1——**浏览器把
旧的 `agent_wasm.js` / `.wasm` 缓存住了**（ES module 缓存很凶）。差点误判成升级失败。
用 `fetch(..., {cache:'reload'})` + 带 query 的 URL 重进才拿到新构建。
**下一个做 wasm 真机验收的人一定会再踩这个**，务必先确认加载的是哪一份
（查 `toolTableJson()` 的条数，或 `typeof host.deleteSession`）。

### 逐条结果

| # | 验收 | 结果 |
|---|---|---|
| 1 | 升级路径：版本 1 的库打开后 journal 一条不丢 | ✅ 版本 **1 → 2**；store 从 `[journal]` 变成 `[images, journal]`；journal **14 条 / 15996 字符，前后一字未动**；`historyJson()` **逐字节相同**（hash `3864121711` 前后一致），重放 6 条消息 |
| 2 | 新建会话两张 store 都在 | ✅ `[images, journal]`，`images` 走 in-line key（`put(record)` 不传 key 成功） |
| 3 | Blob 往返 | ✅ 存 16 字节 PNG 头 → 关连接 → 重开 → 读回**逐字节相同**；`instanceof Blob` 为真（不是 base64） |
| 4 | `deleteSession` 后同 id 重开是空会话 | ✅ 库消失；重开「重放出 0 条消息」，不是报错也不是旧历史；`sessionId()` 变 `null` |
| 5 | 反向锁：有连接开着时 reject 而不是挂住 | ✅ **2 ms** 内 reject，消息为「删不掉：这个会话的库还有别的连接开着……注意删除请求还挂在浏览器里，等最后一条连接关掉它仍会生效」 |
| 5b | 「reject ≠ 没删」这句话是不是真的 | ✅ 重载页面（关掉所有连接）后，那条挂着的删除请求**自己生效了**，库消失 |
| 6 | 两个标签页 + `onversionchange` 自保 | ✅ B 标签页握着同一个库的 Rust 连接，A 调 `deleteSession` **6 ms** 完成、没被 blocked |

### 升级后还能写（issue 没要求，但不验等于没验完）

`compat-1` 升到版本 2 之后继续对话：journal **14 → 20 条**，且模型正确回忆起了
**升级前**的对话内容（「我们先用一句话解释了什么是原子状态引擎，然后我用工具查了
当前页面标题…」）。这是重放**语义正确**的证据，比字节相同更强——字节相同也可能是
两边一起坏掉。

### 顺带证实了一条设计代价（`onversionchange` 引入的新失败形态）

复核实现时标记过：版本恒为 1、从不升级、没有删除，所以 `versionchange` 从来不会触发；
128 之后会了。A 删库 → B 的连接自己关 → **B 此后写不进去**。

实测：**错误响得很清楚**，不是静默——

```
[store_error] IndexedDB 会话存储 IO 失败：KV 存储操作失败：
JsValue(InvalidStateError: Failed to execute 'transaction' on 'IDBDatabase':
The database connection is closing.
```

`db.rs` 里 `close_on_versionchange` 那句「错误走 `store_error` 出口」是实的，不是托词。

**但留一条给 [129](129-page-image-manager.md)**：这一轮的 `TurnStatus` 仍是
`Done { truncated: false }`，UI 上对话照常进行，**只有日志里有那行错误**。
用户会看到一个工作正常、刷新后全没的会话。持久化是 fire-and-forget，
不该因为存储坏了就掐断在飞的一轮——但 `store_error` 值得比一行日志更显眼。
