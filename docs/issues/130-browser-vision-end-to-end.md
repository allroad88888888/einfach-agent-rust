# 130 接起来：`web:source/vision` 端到端

**里程碑** M14 · **依赖** [122](122-page-declared-tools.md) + [124](124-transient-source-in-browser.md) + [127](127-agent-host-inspect-image.md) + [129](129-page-image-manager.md) · **模型** sonnet · **独测** 真机 · **状态** 完成（真机已验收，见文末）

## 目标

把前面五条接成一条模型真的会用的工具。

**这条本身几乎没有新代码**——它的价值全在「五个各自验过的零件合起来还对不对」。

## 做什么

### 0. ⚠️ 先把 `vision` 配置补进页面（127 真机验收时发现的接缝）

`www/index.html` 的配置 JSON **没有 `vision` 段**，于是页面建出来的 `AgentHost`
永远 `vision: None`，[127](127-agent-host-inspect-image.md) 的 `inspectImage`
必然返回 `not_configured`。127 的改动范围排除 `index.html`、129 的 issue 没提这件事,
**这一格今天没人认领**——归本条。

加两个输入框：Kimi 的 `base_url` 与 `api_key`（后者 `type=password`，
横幅只许显示长度，111 契约第 4 条），拼进 `new AgentHost(...)` 的配置 JSON 顶层
`vision` 对象里。形状见 127 实做记录。

**不做这一步，本条的所有验收都会稳定失败，而且看起来像识图坏了。**

### 1. 页面声明 `web:source/vision`

走 [122](122-page-declared-tools.md) 的声明入口。名字前缀 `web:source/` 是**故意的**：
它自动激活 transient-source 那一整套（[119](119-browser-host-capability-decision.md) §三）。

`description` 与 `schema` **照抄 `vision_inspect_spec()`**（`vision_inspect.rs:102`），
只改两处：
- 名字
- 「只接受本地上传返回的链接（形如 `/uploads/<id>`）或本机相对路径」→
  浏览器里没有「本机相对路径」这回事，只留链接那一种

⚠️ **不要重写这段描述。** 它是进 prompt 的字节，而且现有那份是真机验过模型能照着用的。
`reversibility` 落 `Irreversible`（调第三方 API 计费），跟 native 那条一致。

### 2. 工具回调里接上

[121](121-js-tool-callback.md) 的回调收到 `web:source/vision` 时：

```
解析 input.image → resolveImage(link)          （129）
  → host.inspectImage(bytes, mime, question)   （127）
  → 返回文本
```

错误分三类往回报，**措辞对齐 `vision_inspect.rs` 已有的错误码**
（`bad_input` / `not_found` / `upload_failed` / `provider_error`），
不要新造一套——两边错误码不一致的话，同一个故障在 CLI 和浏览器里长得不一样，
排查时会以为是两个 bug。

## 验收

**真机，一次完整的对话。**

- 上传一张写着可辨认内容的图 → 跟模型说「看看这张图 `/uploads/<id>`」→
  **模型自己调 `web:source/vision`** → 答对图里的内容。
  「模型自己调」是关键：不是页面替它调，是它读了工具描述之后决定调。
- **追问第二次**：「再看看图里 XX 部分」→ 模型**再调一次**同一个链接 →
  仍然成功。这条证明 119 §五-2 那个「会话级、不能用完就删」的决定真的落地了。
- **transient-source 三条**（跟 [124](124-transient-source-in-browser.md) 同款，
  这次是真图不是 echo）：
  1. 历史里那条 `ToolUse` 的入参是 `{"transient_source":"redacted"}`
  2. 历史里那条 `ToolResult` 是 `[transient_source_result_redacted]`
  3. **`/uploads/<id>` 这个链接本身可以出现在历史里**（它是用户说的话），
     但**图片字节一个都不在**——用 DevTools 翻 journal 那张 store 确认
- **刷新之后**：会话从 journal 重放，图还在，还能再识别一次。
- **错误路径**：给一个不存在的 `/uploads/xxx` → 模型收到 `not_found` 类的
  `is_error` → **自己纠正**（问用户要图，或者说图没了），不是反复重试同一个链接。

## 注意

- ⚠️ **这一轮会是全价重编码**。one-shot 请求的安全重编码
  （`provider_call.rs:176-194`）会让第 1 层判读报 `Intentional` 漂移而不是 `Reuse`。
  **那是预期内的**，[124](124-transient-source-in-browser.md) 已经登记过。
  验收时看到漂移告警不要当成 bug——但也**不要因此去关掉告警**。
- ⚠️ **别在这条里改 Rust。** 如果接的时候发现需要动 Rust，说明前面某条没做完
  ——回去补那条，不要在这里打补丁。这条 issue 的改动面应当只有
  `www/` 下的 JS 和工具声明 JSON。
- 图片 token **随面积长，不是固定开销**（`docs/IMAGES.md` §一：4x/10x/24x 三档
  分别 16/48/248 token）。2 MiB 的图外推是万级 token。**这笔钱花在 Kimi 那次
  独立调用上，不进主对话的上下文**——这正是 vision 做成工具而不是内容块的理由。
  验收时顺手记一下真实 token 数，写进实做记录。

## 实做记录（2026-08-12）

**零 `.rs` 改动**（`git status` 里没有任何 `.rs`），全在 `crates/agent-wasm/www/`。
issue §注意那条「发现要动 Rust 说明前面某条没做完」的检验通过了：三件事都只用到
前置件已经导出的东西，一处补丁都没打。发现的两个缺口只报告不动手，见文末。

| 文件 | 变化 | 行数 |
|---|---|---|
| `www/page-tools.js`（新建） | 页面这一侧的工具表：**声明什么 + 怎么执行** | **129** |
| `www/vision-tool.js`（新建） | `web:source/vision` 一条工具的执行体 + 错误码翻译 | **169** |
| `www/transcript.js`（新建） | 对话区与事件流的渲染：runner 事件 / 重放历史 → DOM | **101** |
| `www/index.html` | 加识图配置两行 + 配置 JSON 的 `vision` 段；三块搬走 | 339 → **247** |

### 拆分（红线 9）：先拆才有地方加

`index.html` **动手前就已经 339 行，是超限状态**。只把工具那块搬走能落到 308——
仍然超。所以切了两刀，都按职责切、不是按行数切：

| 那一件事 | 落点 | 判据 |
|---|---|---|
| 页面声明什么工具、这些工具怎么执行 | `page-tools.js` | 122 实做记录第 4 条那条判断的直接后果：**谁实现谁声明**。描述和实现拆到两处住，描述写歪没人报错，模型照着错描述用工具 |
| `web:source/vision` 这一条怎么跑、失败怎么说话 | `vision-tool.js` | 它独有的东西是**错误码翻译**（跨 crate 契约的一半），跟「页面有哪些工具」是两件事。依赖方向单向：`page-tools` → `vision-tool` |
| runner 事件与历史怎么画进 DOM | `transcript.js` | 只往两个元素里写字，不认识 `AgentHost`/工具/网络；反过来 `index.html` 也不再直接碰 `#transcript` 的 DOM |

`index.html` 剩下的一句话职责：**页面骨架 + 把各模块接到 DOM 事件上**。
`assistantNode` 这个流式状态跟着渲染搬进 `transcript.js`，不再是页面级变量——
调用方只看得见 `appendUserTurn` / `renderHistory` / `endTurn` 三个入口。

依赖是一条链，没有环：`index.html` → {`page-tools` → `vision-tool`, `transcript`,
`image-manager` → `image-store`}。这跟 129 那次「数据层 / DOM 胶水」是同一个路子。

### 1. §0 的 `vision` 配置补进页面

`fieldset 1` 加两行输入框（`#visionBaseUrl` 默认 `https://api.moonshot.cn/v1`、
`#visionKey` 是 `type=password` + `autocomplete=off`，跟主 `#apiKey` 同款），拼进
`new AgentHost(configJson, …)` 的配置 JSON **顶层 `vision` 对象**——形状照 127 实做
记录第 2 条：`{"base_url":"…","api_key":"…"}`，`model` 不填走 `DEFAULT_KIMI_MODEL`
（`kimi-k3`）。

**跟主对话 provider 完全独立**，不做「主 provider 是 kimi 就复用主 key」的隐式回退：
那是 `vision.rs` 模块文档拍过的，也是 native `resolve_vision` 一直的做法。两个字段
留空合法——`KimiVisionConfig::parse` 返回 `None`，建宿主照样成功，只是识图那一刻
reject `not_configured`。

**横幅只显示长度**（111 契约第 4 条）：`…key=${host.keyLen()} 字符，识图 key=${$('visionKey').value.length} 字符`。
主 key 走 Rust 的 `keyLen()`，识图 key 没有对应的宿主方法（`AgentHost` 只暴露主 key
那一个），所以直接取输入框的 `.value.length`——**是长度不是 key**，一样满足契约。

### 2. 声明 `web:source/vision`

走 122 的 `PAGE_TOOL_DECLARATION` 模块级常量（搬进 `page-tools.js` 之后仍然是模块级
常量字符串，不是现拼的字面量——红线 11 那条责任没有松动）。名字前缀 `web:source/`
是故意的：它自动激活 transient-source 那一整套（119 §三），`turn.rs` 的分流按名字
前缀走（124），页面这边一行都不用写。`reversibility` 落 `irreversible`，跟 native
`vision_inspect.rs:66-68` 同一条理由（调第三方 API 计费，undo 不该重放）。

**description 与 schema 是照抄的，机器校验过**：写了一段脚本，把
`vision_inspect_spec()` 的 description 字面量按 Rust 的 `\` 续行规则还原成一整行，
跟这份声明里的字符串做全等比对——**只差「或本机相对路径」那六个字，其余逐字节相同**。
schema 同理。

⚠️ **「或本机相对路径」在原文里出现两次，两处一起去掉**：description 里的
「…（形如 /uploads/<id>）**或本机相对路径**，不接受公网 URL」，以及 schema 里
`image` 那条的「本地图片链接（/uploads/<id> **或相对路径**），必填」。issue 只点了
第一处，但只去一处等于仍然在 schema 里告诉模型可以传相对路径——浏览器里没有本机
文件系统，它照做就是一次必然失败的调用。这是同一处修改的两个出现位置，不是多改。

**另外两条声明（`web:host/callback-probe`、`web:page/viewport`）逐字未动**，也是
机器比对过的（跟 `git show HEAD:…/index.html` 里那份做 JSON 全等）。红线 11 的
代价照实说：**工具表的字节这次又变了一遍**（多一条），旧会话第一轮的前缀缓存会再断
一次——121 实做记录第 4 条、122 第 7 条各预告过一次，这是第三次。

### 3. 工具回调里接上

`page-tools.js` 的回调命中 `web:source/vision` → `vision-tool.js::runVisionTool`：

```
inputJson → parseInput → resolveImage(image)   （129，绑定当前会话）
                        → inspectImage(bytes, mime, question)   （127）
                        → 识别文本
```

`inspectImage` 在工具回调里调是**正常用法**，不是钻空子——121 实做记录第 2 条那张
借用表明写「`cancel()`/`toolTableJson()`/`keyLen()`/`inspectImage()` 安全：这四个不碰
`live`」，而且那张表原样进了生成的 `.d.ts`。`host` 从 `index.html` 的模块级 `let` 经
一个闭包传进来，取值发生在回调真被调用的时候。

**入参是认领之后的真值**，不是历史里那个 `{"transient_source":"redacted"}` 占位符
——124 实做记录「认领不是协议形式，是拿到真入参的唯一路径」那条已经把这件事钉死在
`turn.rs::drain_transient_source` 里，页面这边什么都不用做。

#### 错误分类：不新造一套

失败一律 `throw new VisionToolError(code, detail)`，`message` 拼成 **`[code] detail`**
——这正是 native 侧模型看到的形状（`agent-runtime/src/tool_exec.rs:50` 把
`ToolError` 拼成 `format!("[{}] {}", err.code, err.message)`）。浏览器这条路的失败走
121 的 reject → `RemoteToolOutput::Failure`，中间没有那一层拼装，所以手工拼同一个
形状；模型在两种形态下看到的失败文本因此逐字同构。

| 来源 | 落成的码 | native 同款位置 |
|---|---|---|
| `image` 缺失 / 非字符串 / 为空 / 入参不是合法 JSON | `bad_input` | `vision_inspect.rs::parse_input` |
| `ImageLinkError.code === 'bad_format'`（不是 `/uploads/` 开头） | `bad_input` | `vision_source.rs::resolve_bytes` 第一条 |
| `ImageLinkError.code === 'bad_id'`（id 不在白名单） | `bad_input` | 同上第二条 |
| `ImageLinkError.code === 'not_found'`（会话里没这张图） | `not_found` | `vision_source.rs::read_uploaded` |
| `inspectImage` reject 且 message 以 `Kimi 图片上传失败：` 开头 | `upload_failed` | `vision_source.rs::upload` |
| `inspectImage` reject 且 message 以 `Kimi 识别请求失败：` 开头 | `provider_error` | `vision_source.rs::chat_completion` |
| message 已经带码（`not_configured：` / `too_large：` / `invalid_response：`） | 原码保留 | `vision.rs` / `parse_content` 自己拼的 |
| 认不出来的 | `provider_error` | 兜底，跟 native 把 chat 阶段一切失败归到它是同一个粒度 |

**一处故意的措辞差异**：native 的 `not_found` 正文用裸 id（`上传的图片不存在：{id}`），
这里用完整链接。模型给出去的是链接，回它链接它才知道是哪一次调用错了；而复述裸 id
要在 `vision-tool.js` 再抄一份 `/uploads/` 前缀常量——**重复一个形状常量比重复一句话
危险得多**（两边被改歪一个，症状是链接解析静默失配）。前缀只有 `image-store.js` 一处
知道。

`too_large` 这条**结构上走不到**：129 的 2 MiB 闸在 `addImage` 里、在开库之前，
存进 `images` 的字节永远 ≤ 2 MiB，所以 `vision.rs` 那道同样是 2 MiB 的闸从这条路
够不着。留着分类是因为它是 `inspectImage` 契约的一部分（页面直接调它时可达）。

### 命令输出（全部前台跑完）

```
$ ls crates/ && git log --oneline -5      # WORKFLOW §四 0
（十个 crate；HEAD = 659b9bb）
$ cargo test --workspace                  # 动手前的基线
exit 0

$ node --check page-tools.js && node --check vision-tool.js && node --check transcript.js
OK（连同 image-store.js / image-manager.js 一并复查，5 个全过）

$ python3 -m http.server 8801   # 起 www/ 自查
index.html 200  page-tools.js 200  vision-tool.js 200
transcript.js 200  image-store.js 200  image-manager.js 200

$ bash scripts/build-wasm.sh --dev
Finished dev profile / ✨ Done in 2.67s / 产物：crates/agent-wasm/www/pkg

$ cargo test --workspace
exit 0

$ bash scripts/check-invariants.sh --all
exit=0（15 条既有的红线 9 行数提示，全部是 crates/agent-{cli,core,mcp,providers,
runtime,server,store}/ 下的存量 .rs；没有一条命中 www/）

$ git status --porcelain
 M crates/agent-wasm/www/index.html
?? crates/agent-wasm/www/page-tools.js
?? crates/agent-wasm/www/transcript.js
?? crates/agent-wasm/www/vision-tool.js
（**没有任何 .rs**）
```

行数复核：`index.html` 247、`page-tools.js` 129、`vision-tool.js` 169、
`transcript.js` 101、`image-store.js` 202、`image-manager.js` 85——全部 ≤300。

### 待真机（主会话跑，页面已全部就位）

本条验收**全部是真机**，这次一条都做不了（无浏览器、无真 Kimi key）。逐条怎么验：

**前置**：填主 provider 配置 + **识图 base_url/api_key**（§0 那两个新框，不填的话
识图稳定 `not_configured`，而且看起来像识图坏了）→ 勾「装上（121）」→ 建宿主 →
开会话 → 选一张写着可辨认内容的图（127/129 用的是 canvas 现画的 `7413` PNG，
16442 字节）→ `#input` 里会自动出现「我上传了一张图：/uploads/up-…」。

| # | 怎么验 | 期望 |
|---|---|---|
| 1 | **主证据**：发「看看这张图 `/uploads/<id>`」 | 事件流出现 `→ 调用宿主工具 web:source/vision  input={"transient_source":"redacted"}` 与 `[tool-callback] ← web:source/vision`（**这一行的 input 是真值**），模型答对图里的内容。关键是**模型自己决定调**，不是页面替它调 |
| 2 | **追问第二次**：「再看看图里 XX 部分」 | 模型**再调一次同一个链接**，仍然成功。这条证明 119 §五-2 那个「会话级、不能用完就删」的决定真的落地了——图还在 `images` store 里 |
| 3 | transient-source ①：历史里的 `ToolUse.input` | DevTools 读 IndexedDB 的 `journal` store，解出那条 `ToolUse` → 入参是 `{"transient_source":"redacted"}`，不是真链接 |
| 4 | transient-source ②：历史里的 `ToolResult.content` | 是 `[transient_source_result_redacted]`，不是识别正文 |
| 5 | transient-source ③：图片字节一个都不在 journal 里 | 翻 journal 那张 store 确认没有任何 base64/二进制块。`/uploads/<id>` 这个链接**可以**出现（它是用户说的话，合法） |
| 6 | ⚠️ **补一条 124 当时做不到的全文断言** | 见下 |
| 7 | **刷新之后** | 会话从 journal 重放，「已上传的图片」列表里那条还在，再问一次还能识别 |
| 8 | **错误路径** | 说「看看 `/uploads/up-doesnotexist`」→ 模型收到 `[not_found] 上传的图片不存在：/uploads/up-doesnotexist` 的 `is_error` → **自己纠正**（问用户重传、或如实说图没了），不是反复重试同一个链接 |
| 9 | 顺手记 token | `turn_guard` 事件的 `prompt/completion/cached`，以及 DevTools Network 里 Kimi 那两跳的请求体大小。图片 token **随面积长**（docs/IMAGES.md §一），但这笔钱花在 Kimi 那次独立调用上、**不进主对话上下文**——这正是 vision 做成工具而不是内容块的理由，值得留一个真实数字 |

**第 6 条（新增，124 点名留给本条的）**：124 实做记录末尾那条教训说，echo 工具
「原样返回入参」，标记会合法地出现在用户提示词和模型终答里，所以**没法用全文搜索
验证**——只能把断言钉在位置上。**识图不一样**：识别结果是**用户和模型都没说过的
新文本**（比如图里那个四位数字之外的某段描述）。所以这一次可以做全文断言：

> 从识别结果里挑一段**只可能来自 Kimi**的文本（不是用户提示词里的词、也不是模型
> 终答里复述过的词），在整份 journal 里全文搜索 → **一次都不该命中**。

这是 transient-source 第一次拿到「全文级」证据面，比三条位置断言更强。挑不出这样
一段文本时（识别结果太短、全被模型复述了）就问一个答案长的问题，比如「详细描述
这张图的排版」。

### ⚠️ 两个需要动 Rust 的缺口（本条只报告，一处未动）

1. **`inspectImage` 的 reject 没有机器可读的错误码。** 它跨 wasm 边界 reject 的是一个
   `js_sys::Error`，**只有 message**。于是 `vision-tool.js` 只能**按前缀认字符串**
   （`Kimi 图片上传失败：` → `upload_failed`、`Kimi 识别请求失败：` → `provider_error`），
   而那两个前缀是 `agent-wasm/src/vision.rs` 里两个 `map_err` 的字面量。
   **改了那两句话，浏览器这边会静默把失败重分类成 `provider_error`**——不报错、不断
   编译、没有测试盯着。这是本条唯一一处「JS 依赖 Rust 的消息文本」的耦合。
   要根治得让 `inspect()` 的 `Err` 带上码（比如 reject 一个带 `code` 字段的对象，或
   统一把 message 拼成 `{code}：{detail}` 形状——它今天已经**部分**这么做了：
   `not_configured` / `too_large` / `parse_content` 转发的那三条有码，上传/chat 那两条
   没有）。**属于 127 的接口面，不在本条改。**
2. 127 与 124 各自留下的两条文档欠账**仍然没人补**（本条同样不碰 `.rs`）：
   `agent_transport::MAX_IMAGE_BYTES` 那边缺一句反向指回 `agent-wasm/src/vision.rs`
   的注释（127 实做记录第 1 条）；`host.rs` 里 `send()` 上方那句「transient-source
   在这个宿主里结构上不可达」的注释在 124 之后就过期了、而 130 落地后更不准
   （现在真有一条 `web:source/vision` 在跑）——124 实做记录「已知的局限」第 2 条
   登记过，至今未补。

## 真机验收（主会话，2026-08-12，Chrome + 真 Kimi key）

**M14 的核心功能跑通了。** 工具表 6 条，`web:source/vision` 由页面声明。
探针图：canvas 现画，白底黑字两行——「紫色犀牛」+「9264」，28508 字节。
**这两样内容我一个字都没在提示词里提过。**

### 主证据：模型自己决定调，并答对了内容

```
user      我上传了一张图：/uploads/up-b2d9…  帮我看看这张图里有什么。
→         调用宿主工具 web:source/vision  input={"transient_source":"redacted"}
←         web:source/vision 返回 34 字节
assistant 纯白背景上居中排列着两行黑色粗体文字：
          第一行：四个汉字"紫色犀牛" / 第二行：四个数字"9264" …
```

「模型自己调」是关键：不是页面替它调，是它读了工具描述之后决定调。27 秒，
`prompt=695 completion=127`。

事件流那行 **`返回 34 字节`** 本身就是证据——34 正是
`[transient_source_result_redacted]` 的长度，**宿主侧看到的已经是遮罩后的**。

### 逐条

| # | 验收 | 结果 |
|---|---|---|
| 1 | 模型自己调、答对内容 | ✅ 见上 |
| 2 | 追问第二次同一链接 | ✅ 再调一次，答出 `9264`——**会话级生命周期真的落地了**（119 §五-2 那条「不能用完就删」的决定在这里兑现） |
| 3 | `ToolUse` 入参遮罩 | ✅ `{"transient_source":"redacted"}` |
| 4 | `ToolResult` 遮罩 | ✅ `[transient_source_result_redacted]` |
| 5 | 链接在历史里、**字节不在** | ✅ `/uploads/<id>` 在 journal（用户说的话，合法）；`images` store 有 1 条 28508 字节，**journal 里零 PNG 数据** |
| 6 | 刷新重放 + 再识别 | ✅ 会话重放、图还在、再识别一次答出「紫色犀牛」 |
| 7 | 错误路径自纠 | ✅ 不存在的链接 → `33 字节（错误）` → 模型说「图片不存在，请检查链接或重新上传」，**没有反复重试同一个链接** |

第 7 条有个值得记的细节：**模型知道是「不存在」而不只是「失败」**。真错误经 one-shot
覆盖进了 prompt，历史里留的才是遮罩版（`[transient_source_error_redacted]`）——
两边各司其职，这正是 transient-source 的设计意图。

### ⚠️ 一个真实的 UX 后果（机制没错，页面缺一块）

刷新之后，重放出来的历史里**模型那段识别回答是
`[transient_source_candidate_redacted]`**，不是它当时说的话。

查过了，**这是设计不是 bug**：`transient_source_completion.rs:52`——一轮消费过 one-shot
素材之后，模型自己那段完成文本也不进持久历史，真文本经 `emit_terminal_candidate`
单独交给宿主。理由直白：模型看过源素材，它的话可能引用了素材。

但对浏览器形态的后果是具体的：**直播时用户看得见答案，刷新后那段对话读起来是一串
占位符。** server 形态下宿主（网关）被期望自己把真候选留在带外，浏览器这个页面
今天只是照着重放结果画。

**不在本条范围**（本条零 `.rs` 改动、也不该改页面的持久化策略），登记给
[132](132-m14-dogfood.md) 的交界处清单，以及 M14 遗留。

### 主会话补的一处：`inspectImage` 的错误码

130 如实报告了一个它没动手修的缺口——`inspectImage` 的 reject 只有 message 没有码，
于是 `vision-tool.js` 只能**按中文散文前缀认字符串**（「Kimi 图片上传失败：」之类），
而那两句是 `vision.rs` 两个 `map_err` 的字面量。**改一句提示语，浏览器侧就把
`upload_failed` 静默重分类成兜底的 `provider_error`**——不报错、不断编译、没有测试盯着。

主会话把缺的三个码补进 `vision.rs`（`upload_failed` / `provider_error` / `bad_input`），
并把「`Err` 的形状是 `<码>：<细节>`」写成 `inspect` 文档注释里的契约；
`vision-tool.js` 那张中文前缀表随之删掉，只留一条正则。**JS 侧的行为在补码前后一致**
（`ALREADY_CODED` 本来就排在散文表后面兜底），改的是「靠猜」变成「靠契约」。
