# 130 接起来：`web:source/vision` 端到端

**里程碑** M14 · **依赖** [122](122-page-declared-tools.md) + [124](124-transient-source-in-browser.md) + [127](127-agent-host-inspect-image.md) + [129](129-page-image-manager.md) · **模型** sonnet · **独测** 真机 · **状态** 待做

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
