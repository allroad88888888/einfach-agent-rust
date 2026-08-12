# 119 决策：浏览器宿主能力的两条缝（通用工具回调 + 图片）

**里程碑** M14 · **依赖** [111](111-wasm-target-decision.md) + [114](114-wasm-host.md) · **模型** opus · **独测** 决策类 · **状态** 已拍板

> ⚠️ **动手前必读这一条。** 下面每一格都是拍过的，实现 issue 里不要重新讨论；
> 真觉得错了，带证据来推翻，别默默改。

## 一、核心判断：这是同一条缝的两个投影

需求最初是两条：**「wasm 不支持图片」** 与 **「同步的 `host_tool::execute()` 改成可
等待 JS Promise 的通用回调」**。核查之后它们是同一件事，且有严格的先后：

> **需求 2 是需求 1 的前提，而需求 2 本身几乎免费。**

证据链：

1. `agent-wasm/src/turn.rs:78` 的 `drain_host_tools` **已经是 `async fn`**，
   第 87 行的 `host_tool::execute(&waiting)` 是这条 await 链上**唯一剩下的同步点**。
   改成 `.await` 就完了——泵、`agent-core`、native 侧一行不动。
2. 图片的**传输层在 wasm 上已经通了**：`agent-transport/src/fetch_upload.rs`（113）
   是完整的 multipart-over-fetch，`upload_image_async` 能跑。
3. 卡住图片的地方仓库自己诊断过，原话在 `agent-transport/src/fetch_client.rs:189-192`：
   `post_json` 是个永远报错的 stub，注释写着「真要让浏览器里的识图工作，**要动的是
   `ToolExecutor` 那条同步缝**」。

## 二、拍板：不 async 化 `ToolExecution`

`ToolExecution::execute` 是同步 `fn`（`agent-tools/src/lib.rs:147`），调用点
`agent-runtime/src/tool_exec.rs:31` 在同步的 `dispatch` 里。把它 async 化要动
core 的派发 + native + `agent-tools` 那几十处调用点。**不做。**

理由跟「M13 遗留」第二条（`agent-mcp` 做成 feature）是同一条：那是把浏览器的平台约束
（单线程、无阻塞原语）推进 `agent-tools` 和 `dispatch`——**平台判断不进核心执行路径**，
红线 12 的精神刚为 MCP 讲过一遍。

**替代方案**：浏览器里 vision 根本不该是 `srv:` 工具，它是 `web:` **宿主工具**。
页面声明它、页面的回调执行它。这正是 M10 能力注入链路，一行新机制都不需要。

## 三、拍板：`web:source/` 前缀白捡了一整套机制

`agent-runtime/src/transient_source_policy.rs:8` 的前缀是 **`web:source/`**——
本来就是为 `web:` 宿主工具设计的。把浏览器识图声明成 `web:source/vision`，自动拿到：

| 白捡的 | 落点 |
|---|---|
| 入参在历史里被换成 `placeholder_input()` | `dispatch.rs:183` |
| 结果在历史里被 `SAFE_RESULT` 遮掉 | `remote_tool_submission.rs` |
| 真正的正文只 one-shot 覆盖进 prompt 一次 | `transient_source_prompt::prepare` |
| 前缀镜像/漂移报告不被 one-shot 正文指纹化 | `provider_call.rs:176-194` 的安全重编码 |

**一个真分叉**：这条路必须调 `submit_remote_tool_result_async`，**不是**
`resolve_remote_tool_async`——后者 `remote_tool.rs:63-68` 显式拒绝 transient-source 工具，
而 `turn.rs:88` 今天调的正是被拒的那个。见 [124](124-transient-source-in-browser.md)。

## 四、拍板：JS 与 Rust 的分工

| | 谁做 | 为什么 |
|---|---|---|
| 选图、存 IndexedDB、发链接、按链接取回字节 | **页面** | 每一步都是 JS 原生的事。进 Rust 要付：agent-wasm 多一批 web-sys feature（`Blob`/`File`/`FileReader`）、`lib.rs` 开头那句「这个 crate 只做装配，不做实现」破口子 |
| 拿到字节之后的 provider 协议（multipart → `ms://` → chat completions → 抽文本） | **Rust** | `fetch_upload.rs` 已经把 multipart 编码、boundary、大小闸、错误分类全写完了；`vision_inspect.rs` 也把 Kimi 那个 `image_url` + `ms://` 的请求体形状写对了。**JS 重写一遍就是两份会漂的实现** |

落地形状：`AgentHost` 多一个 `inspectImage(bytes, mime, question) → Promise<string>`，
页面在工具回调里调它。**存储形状完全不进 Rust 的契约。**

## 五、拍板的四条参数（用户 2026-08-11 定）

| # | 决定 | 理由 / 代价 |
|---|---|---|
| 1 | **浏览器侧单张图上限 2 MB** | native 是 100 MiB（`MAX_IMAGE_BYTES`，Moonshot 的限制）。IndexedDB 配额是整个 origin 共享的，一张 100 MiB 的图能把配额吃光。**两个数字不是同一个常量**，不要把 native 那个直接搬过来 |
| 2 | **会话级生命周期**：图片活到会话被删为止 | 模型很可能对同一张图追问第二次（「右下角那个税号是多少」）。**不能用完就删。** 代价：要新造一条今天不存在的路径——`AgentHost.deleteSession(id)` + `deleteDatabase`；用户不删就一直涨 |
| 3 | **放同一个库**（`agent-session-<id>`），新增 `images` object store | 版本号 1→2。删会话时一起没，不需要记着删两个地方 |
| 4 | **不调 `navigator.storage.persist()`** | 第一版接受被浏览器驱逐。降级是干净的：`vision_inspect.rs` 已有 `not_found` 错误码，模型看到 `is_error` 自纠 |

## 六、一句必须先改准的措辞

`agent-tools/src/vision_inspect.rs` 模块文档现在写着图片字节
「不落消息历史、不进 prompt、**不进任何持久化**」。

**最后半句不准确**：server 形态其实也把字节落在 `<dir>/<id>` 了
（`agent-server/src/http/uploads.rs`）。那句话真正的意思是
**「不进会话历史 / prompt / journal」**。

图片进 IndexedDB 的独立 object store 完全遵守这条，但文档得先说准——
否则下一个人会拿那句话把整个方案否掉。见 [131](131-vision-persistence-wording.md)。

## 七、三条已知的坑（实现 issue 各自负责，这里只登记）

1. **重入**：`host.rs:4-18` 那条借用纪律——`send()` 在整轮期间持有 `live.borrow_mut()`，
   **工具回调正是在这个借用之内被调用的**。今天那条纪律是写给事件回调的（「只读、只画」），
   事件回调没人想在里面干活；工具回调天然想干活，页面里一句 `await host.send(...)`
   就是 `already borrowed` panic。归 [121](121-js-tool-callback.md)。
2. **取消与超时**：今天 execute 瞬时返回，「工具执行期间取消」这个状态根本不存在。
   可等待之后一个 JS Promise 能挂 30 秒。`sweep_remote_tool_deadlines_async` 已经有了
   但 `turn.rs` 没调。归 [123](123-host-tool-deadline.md)。
3. **红线 11 的责任推给页面**：工具表进 prompt 最前面，页面每次刷新给的声明 JSON
   必须逐字节一样，否则前缀缓存每次刷新全断。`with_host_tools` 只帮你按名字排序，
   帮不了字段顺序和描述文案。归 [122](122-page-declared-tools.md)。

## 八、验收手段的现实（每条实现 issue 都受这条约束）

`agent-wasm` 是**独立 workspace + wasm32 目标**，`cargo test --workspace` 覆盖不到它。
所以本里程碑的验收只有三种形态，写验收时必须挑明用的是哪种：

| 形态 | 手段 | 适用 |
|---|---|---|
| native 可测 | `cargo test --workspace` | 纯逻辑（`agent-transport`/`agent-tools`/`agent-runtime` 里的部分） |
| 编译期 | `bash scripts/build-wasm.sh` | wasm 绑定对不对得上 web-sys |
| 真机 | Chrome + 真 key | 运行时行为，M13 就是这么验的 |

**「native 可测」优先**：能摘到 native 侧用纯函数钉住的，就不要留到真机去看。
[126](126-vision-pure-logic.md) 存在的唯一理由就是这条。
