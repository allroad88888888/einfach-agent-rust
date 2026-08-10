# 111 恢复 wasm 目标：核心编进浏览器直接跑（**决策**，取代决策 10）

**里程碑** M13 · **依赖** — · **模型** opus · **独测** 决策类

用户拍板（2026-08-10）。这条**推翻 [ROADMAP](../ROADMAP.md) 决策 10**「砍掉 wasm 目标，
Tauri 内嵌 server」。决策 10 的两条理由现在都不成立，下面逐条给证据——**不要凭印象重新
讨论，先看这里的数字**。

## 决策

**wasm 是第三种宿主形态**：独立跑 / 宿主子进程 / **浏览器内**三者并存。
不替代决策 12 的「`agent-server` 是库」，`agent-server-bin` 仍是众多宿主之一。

浏览器形态下的裁剪：

| | 怎么处理 | 理由 |
|---|---|---|
| `agent-mcp` | **不编** | stdio 在浏览器不存在。浏览器够得着的 MCP 由前端自己连并注入成 `web:mcp-<server>/<tool>`，是 [HOST-CAPABILITIES](../HOST-CAPABILITIES.md) §七 早就定的方向 |
| `agent-tools` 的 `srv:` shell/fs specs | **不声明** | 那些是纯数据（`pub fn shell_spec() -> ToolSpec` 只是转发），不声明即可，模型压根不知道有它 |
| `agent-transport` | **换 fetch 实现** | 见 113 |
| `RunnerCtx.fs: ToolExecutor` | **开注入接缝** | 见 112。这是唯一的结构性改动 |

## 决策 10 为什么不成立了

### ① 「provider 不用维护两套」与代码不符

`agent-providers/Cargo.toml` 的依赖只有 `agent-core` + serde + serde_json——**里面没有任何
HTTP 客户端**。它只管把料单编成请求体、把响应解回事件，IO 全在 `agent-transport` 一个
crate 里（那里有全仓唯一允许依赖 ureq 的镜像约束）。

所以要维护两套的从来不是 provider，是 **transport 一个已经隔离好的 crate**。

### ② 浏览器侧的 transport 更薄，不更厚

`agent-transport/src/read_loop.rs` 那 165 行读线程 + `mpsc::sync_channel` + 两个独立超时
旋钮，存在的**唯一理由**是 ureq 的阻塞 `read` 没有外部中断句柄——`lib.rs` 自己把它记成
「不优雅但可测」的取舍，事故记录在 022/023。

`fetch` 原生给流式响应体，`AbortController` 原生就是那个中断句柄。这一整坨在浏览器侧
不需要存在。

### ③ 前提已实测：三家模型都放行浏览器直连

决策 10 当时没验这一条。2026-08-10 实测（`OPTIONS` 预检 + 带 `Origin` 的 `POST`）：

| provider | 预检 | `access-control-allow-origin` | 放行 `authorization` |
|---|---|---|---|
| DeepSeek `api.deepseek.com` | 200 | 回显请求 origin | ✅ |
| Kimi `api.moonshot.cn/v1` | 204 | 回显请求 origin | ✅ |
| GLM `open.bigmodel.cn/api/paas/v4` | 200（`max-age=3600`） | 回显请求 origin | ✅ |

三家都回显任意 origin。`POST` 返 401 只是没带 key，**说明请求本身穿过了 CORS**。

### ④ 决策 16 的让步在这里变成 fit

决策 16 记的是「store 是 `Rc<RefCell>` 不 `Send`，HTTP 在别的线程，所以必须有
`ProviderRequest` 这份能带走的东西」。那是为原生多线程付的代价。
**wasm 默认单线程**，这条约束在浏览器里不再是让步。

## 代价（照实记，不许在后续 issue 里假装没有）

1. **`RunnerCtx.fs: ToolExecutor` 是 concrete struct**（`agent-tools/src/lib.rs:149`），
   `new()` 里要 canonicalize 一个真实目录、不存在就报错。浏览器没有文件系统，必须开注入
   接缝，见 112。
   > ⚠️ 本条原写「**这是本里程碑唯一的结构性改动**」，**已被 113 证伪**：wasm 上没有线程，
   > 而 `io_thread` 同时扛着 029 的并行、`sync_channel(0)` 的会合背压和「放弃不 join」，
   > 是第二处结构性改动，见 [115](115-wasm-io-without-threads.md)。写本条时低估了，
   > 记在这里免得后来人以为只有一处。
   > 顺带一笔文档欠账：[ARCHITECTURE](../ARCHITECTURE.md) §各包边界写着「mock 一个 tool
   > executor」——按当前结构那个接缝**其实不存在**，mock 只能靠给临时目录。112 一并开出来。
2. **`Instant` / `SystemTime`**：`PendingRemoteTool` 的 `deadline` 用的是绝对时刻，wasm 上
   要垫 `web-time`。
3. **多一个编译目标**要长期维护——这是决策 10 当初想省的，现在明确选择付这笔钱。
4. **key 在浏览器里**。CORS 通不等于 key 能随便放：定为**每个用户一把自己的 key**
   （按人发、可单独吊销、用量可归因）。共享一把塞进前端等于谁都能抠出来——**这条写进契约，
   不留给实现者临场决定**。

## 顺带消解的两件事

这两条不是本 issue 要做的，是决策成立后**自动不存在**的，记下来免得后面有人去解：

- **远端工具 600s 截止线**（`ctx.rs` 的 `DEFAULT_REMOTE_TOOL_TIMEOUT`）：宿主与核心同进程，
  够不到。「HITL 等人会不会超时」不再是问题。
- **`tool_claim` 的 CAS、epoch 防迟到、`sweep()` 补漏**：它们是为跨网络准备的，同进程下
  仍在 runtime 里但永远不会触发。**不要因此删掉它们**——server 形态还要用。

## 范围

本 issue 只产出决策与文档，不写实现代码：

1. `ROADMAP.md` 决策 10 划掉并标「被 26 取代」，新增决策 26（照决策 14 的既有写法）。
2. `ARCHITECTURE.md` 两处同步：§各包边界里 `agent-core` 那段的「wasm 目标已砍」括注；
   文末「不做 wasm 编译目标」整段。
3. 本 issue 与 112 / 113 / 114 登记进 `issues/README.md` 的 M13 段。

## 验收（可判定）

- 过期表述不再以**现行结论**的身份出现——只查这两个文件，**不要全 `docs/` 扫**：
  `rg '目标已砍|^不做 wasm 编译目标' docs/ARCHITECTURE.md docs/ROADMAP.md` 无命中。
  （全目录扫会命中本 issue 自身——它必须引用旧表述才说得清取代了什么——以及 ARCHITECTURE
  里「取代原先……」那句说明。第一版就这么写错了，条件不可满足。）
- `ROADMAP.md` 里决策 10 是划掉状态且指向 26；决策 26 存在且写明「三种形态并存」。
- 决策 26 的理由栏里，①②③④ 四条证据都在（不是只写结论）。
- `issues/README.md` 有 M13 段，四个 issue 的依赖链可读。

## 注意

- **不碰任何 `crates/` 下的代码。** 本 issue 是纯决策，实现全在 112–114。
- 仓库当前有另一条线（M12 上下文压缩，issues 095–110）在改 `agent-core/src/command/` 与
  `agent-runtime/src/{child_outcome,provider_call}.rs`。112 要动的是
  `agent-runtime/src/ctx.rs` 与 `agent-tools/`，**开工前先确认没和那边撞同一个文件**。
