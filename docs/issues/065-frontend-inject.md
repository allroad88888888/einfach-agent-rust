# 065 前端：建会话时注入 capabilities

**里程碑** M10 · **依赖** 061（协议形状） · **模型** sonnet · **独测** — · **状态** 完成（2026-08-04）

前端线的第一块，**只做「把声明发出去」**，不碰执行（066）、不碰 MCP（067）。
接缝见 [HOST-CAPABILITIES.md](../HOST-CAPABILITIES.md) §四。

## 范围（`packages/web/`）

1. **一个「本前端有哪些能力」的声明源**（新模块，如 `src/capabilities.ts`）：
   导出一份 `CapabilityTool[]` / `CapabilitySkill[]`，**先放一两个真能跑的示例工具**
   （如 `web:demo/page-title` 读 `document.title`——它只有前端干得了，正好给 068 真机用）。
   名字必须 `web:` 前缀，`reversibility` 该标 `"pure"` 的标上。
2. **`api.ts` 的 `createSession` 带上 `capabilities`**（协议照 061 定的形状）。
   **不传时行为不变**——保留「不声明也能用」的路径。
3. **类型**：优先用 `packages/protocol` 生成的类型；若 061 没把 `Capabilities` 挂 ts-rs 导出，
   **报告**（别在前端手写一份会漂移的镜像类型——那正是 032 建生成链路要避免的事）。

## 验收（可判定）

- 建会话的请求体里带上 `capabilities.tools`，会话正常建起来（201/200）。
- 不带声明的旧路径仍然可用（保留一个开关或参数）。
- `pnpm -r typecheck` + `pnpm --filter web build` 绿。
- 声明里的名字都过 061 的校验（拿一个故意写错的 `srv:` 名字试一次，server 该 400——
  这条顺带验证了前后端对同一份规则的理解一致）。

## 注意

- **只做声明，不做执行**。模型真调用它会怎样是 066 的事；本 issue 落地后
  「声明了但没人执行」是**已知中间态**（060 修完后不会挂死，会拿超时的 `is_error`）。
- **不要碰 Rust 侧**。协议不够用就**报告**，不要自己改 server。
- **与 [066](066-frontend-tool-exec.md) 都会碰 `api.ts`** ——若两者并行，
  你只加 `createSession` 的参数，**不要动** `sendToolResult` 相关（那是 066 的）。
- 收工验证前台跑完（WORKFLOW §四 -1）。

## 实做记录（2026-08-04）

**产出**（纯前端，`packages/web/`；未碰 `crates/`，未碰 `src/mcp/`（067 的地盘），
未加任何 `sendToolResult` 相关（066 的地盘））：

| 文件 | 行数 | 干什么 |
|---|---|---|
| `src/capabilities/index.ts`（新增） | 96 | **声明源**：登记、汇总成 `Capabilities`、按名字查实现 |
| `src/capabilities/demo-tools.ts`（新增） | 57 | `web:demo/*` 这一组示例工具的**声明 + 实现** |
| `src/api.ts`（改） | 87 | `createSession` 多一个可选参数 |
| `src/main.ts`（改） | 56 | 建会话那一行把声明传进去 |

拆成两个文件而不是一个 `capabilities.ts`：「示例工具是什么」是一个业务点，
「怎么汇总/怎么查实现」是一个抽象，塞一个文件里就同时占两个层面（`one-file-one-thing`）。
`src/capabilities/index.ts` 让下游 `import ... from "./capabilities"` 不用知道内部划分。

### 声明源的形状

```ts
registerWebTool(tool: CapabilityTool, impl: WebToolImpl): void   // 声明+实现一起进来
webCapabilities(): Capabilities | undefined                      // 这次建会话要发的东西
findWebTool(name: string): WebToolImpl | undefined               // 066 的入口
isWebToolName(name: string): boolean                             // 发请求前自己先拦一道
type WebToolImpl = (input: unknown) => string | Promise<string>
```

三条判断：

1. **声明与实现不许分家**——`registerWebTool` 一次收两个。只声明不实现 = 告诉模型
   「我有这个能力」然后在它真调用时给错误。示例工具在模块加载时自登记，`demoTools` 和
   `demoToolImpls` 名字对不上就当场抛（不留到模型真调用那一刻）。
2. **`webCapabilities()` 在什么都没声明时返回 `undefined`**，`createSession` 于是落回
   「请求体逐字节还是 `{}`」那条老路——§四的向后兼容不是靠 server 宽容，是这边真的不发。
3. **重名与坏前缀当场抛**：坏名字会让整个 `POST /sessions` 被 061 400 掉、会话根本建不
   起来。067 的失败隔离条款要求它翻译完 MCP 工具名自己先过 `isWebToolName`。
   （逐条比过 061 的 `http/capabilities/validate.rs`：前缀 `web:`/`desk:`，其后只准
   `[A-Za-z0-9_/-]`、总长 ≤128 —— `web:demo/page-title` 的 `-` 和 `/` 都在白名单里。）

### 示例工具（两个，都 `"pure"`）

| 名字 | 实现 | 为什么是它 |
|---|---|---|
| `web:demo/page-title` | `document.title`（空标题回一句人话，不回空串——空 `content` 到模型那边跟「工具没返回」分不开） | 068 §一要的就是这种「只有前端干得了」的任务 |
| `web:demo/viewport` | `JSON.stringify({width, height, dpr})` | 第二个样本，顺带给 061 的按名排序两条数据 |

`schema` 用无参写法 `{"type":"object","properties":{},"additionalProperties":false}`，
跟 Rust 侧无参工具（`agent-tools/src/command_discovery_specs.rs`）同一个写法。
标 `"pure"`：只读 DOM/window，不写任何东西，`/undo` 越过它们不需要问人（§五）。

### 类型来源：**生成的类型，零手写镜像**（过程如实记）

动手时 061 还没落地（`CreateSessionRequest` 只有 `id`/`session_path`，Rust 侧没有
`Capabilities` 可生成），先按 issue 的授权写了一份**本地最小接口**
（`src/capabilities/wire.ts`，只抄 §四用得上的字段 + 一句「待 061 替换」）。
干到一半 061 在工作区落地（`crates/agent-server/src/http/capabilities/`，未提交），
`packages/protocol/src/generated/Capabilit*.ts` 四个文件生成出来、并已从
`packages/protocol/src/index.ts` 导出——**当场把本地定义全删，改从 `@agent/protocol` 导入**。
最终仓里没有任何一行手写的 `capabilities` 形状（决策 2 / 032 的初衷）。

对过的两处形状（本地那版猜的和生成的一致，没踩坑）：

- `CapabilityReversibility` 是**小写** union（`"pure"|"reversible"|"irreversible"`），
  跟下行 `ToolCallRequest.reversibility` 那个大写的 `Reversibility` **不是同一套拼法**——
  一个是宿主报进来的，一个是 core 落盘/推事件用的。061 的模块注释和 protocol 入口注释
  都把这条写明了，前端不需要做任何转换。
- `Capabilities.tools`/`.skills` 都是**可选**的（`ts(optional)`），`webCapabilities()`
  因此只在非空时才带上对应字段。

**并发擦碰（如实记）**：删 `wire.ts` 那一刻 067 已经从 `../capabilities/wire` 导入了
`CapabilityTool`，workspace typecheck 当场红。先补了一个**纯转发** shim（一行
`export type ... from "@agent/protocol"`，不含任何形状）保住绿；随后 067 自己改成直接
从 `@agent/protocol` 导入，确认全仓无引用后把 shim 删掉。**没有动过 `src/mcp/` 一个字**。

### `createSession` 的新签名

```ts
export async function createSession(capabilities?: Capabilities): Promise<string>
```

省略参数 → 请求体逐字节还是 `{}`（这个可选参数本身就是「不声明也能用」的那个开关）。
`main.ts` 传 `webCapabilities()`。**`sendToolResult` 相关一个字没动**（066 的）。

### 真实验证（前台跑完，真实输出）

```
$ pnpm -r typecheck
Scope: 3 of 4 workspace projects
packages/protocol typecheck: Done
packages/web typecheck: Done

$ pnpm --filter web build
✓ 17 modules transformed.
dist/index.html                  1.13 kB │ gzip: 0.61 kB
dist/assets/index-Ct4-gWUj.css   3.94 kB │ gzip: 1.41 kB
dist/assets/index-C-Z8ALT5.js   10.94 kB │ gzip: 4.92 kB
✓ built in 85ms

$ bash scripts/check-invariants.sh <本次改动的四个文件>
红线检查通过
```

**真浏览器（Playwright + 真实构建产物）——两条验收都是抓真实请求体，不是读代码**：

1. **带声明这条**：写了个假 server 同源托管 `packages/web/dist`（模拟 `AGENT_STATIC_DIR`
   的形状，**不经 vite dev 代理**），把 `POST /sessions` 收到的 body 原样落盘。
   浏览器打开页面后落盘的就是：

   ```json
   {"capabilities":{"tools":[
     {"name":"web:demo/page-title","description":"…","schema":{"type":"object","properties":{},"additionalProperties":false},"reversibility":"pure"},
     {"name":"web:demo/viewport","description":"…","schema":{"type":"object","properties":{},"additionalProperties":false},"reversibility":"pure"}]}}
   ```

2. **不带声明的旧路径**：同一页面里 `await api.createSession()`（不传参）→ 落盘的 body
   是 `{}`（2 字节），假 server 照常回 201 + `{"id":...}`，前端拿到 id。

3. **示例工具真的能跑**（真浏览器里调的是源码本身）：把 `document.title` 改成
   `"065 真机验证页"` 之后

   ```
   pageTitle()                              → "065 真机验证页"
   findWebTool("web:demo/page-title")({})   → "065 真机验证页"   ← 066 会走的这条路
   viewport()                               → {"width":1200,"height":829,"dpr":1}
   findWebTool("web:demo/nope")             → undefined          ← 066 要把它翻成 is_error
   webCapabilities().tools                  → web:demo/page-title (pure), web:demo/viewport (pure)
   ```

**没做的一条**：真 agent-server 的联调（201 与「`srv:` 名字该 400」）留给 068。理由：
061 虽已在工作区落地，但 crates/ 正被 061/062 的会话并发改着，这时候起 cargo 只会
和它们抢 target 锁，而且那条 400 是 061 自己的测试面
（`crates/agent-server/tests/http_capabilities_declaration.rs`）。**没有为此改任何前端代码。**

### 留给 066 / 067 的接线点

- **066（执行）**：`tool_executing` 帧的 `request.tool` → `findWebTool(name)` → 调用 →
  `POST /tool_result`。**返回 `undefined` 必须回传 `is_error`，不要沉默**（060 之后不回传
  只是等到超时，回传错误能让模型当场自纠）。`WebToolImpl` 的返回值就是 `content`，抛异常
  = 这次调用失败。**`api.ts` 里 `createSession` 之外的东西 065 一个字没动**，加
  `sendToolResult` 不会撞车。
- **067（MCP）**：`tools/list` 翻译完逐个 `registerWebTool(tool, impl)`（`impl` = 转发一次
  `tools/call`），**必须赶在 `main.ts` 调 `createSession(webCapabilities())` 之前**——声明
  只有建会话这一次机会（§三：不做运行时增删）。所以 067 唯一要改的是 `main.ts` 那一行的
  调用顺序（先 await 连 MCP、再建会话），`src/capabilities/` 两个文件一行都不用动。
  写记录时 067 已经在用这个口子了（`src/mcp/register.ts` → `registerWebTool`）。
- **068（真机）**：`web:demo/page-title` 已经能返回真实的 `document.title`，
  用 `AGENT_STATIC_DIR` 托管 `packages/web/dist` 即可（curl 打本机记得 `--noproxy '*'`）。

### 已知中间态

「声明了但没人执行」——066 落地前，模型真调用 `web:demo/*` 会等到超时的 `is_error`
（060 修完之后不会挂死）。这是 065 的范围条款，不是缺陷。

## 073 追记：什么时候**不该**带声明（契约，前端代码不用改）

073 把注入的声明搬进了会话状态（建会话时 journaled 写一次，恢复时自动回来）。于是多了
一条对**所有**注入方成立的规则：

> **一个 chatid 只有在它还没有任何历史的时候才可以带 `capabilities`。**
> 有历史还带 → **400，错误码 `session_has_history`**（不是通用 `bad_request`）。

判断办法（推荐「先查再建」）：`GET /sessions/{id}` → **404** 就带声明建、**200**
（`alive` / `dormant` / `dead`）就不带。`dormant` 是 073 新增的一态：registry 里没有、
但磁盘上有它的会话文件，也就是下一次 POST 会走恢复的那种情况。

**本前端天然合规，`packages/web/` 一个字都不用改**（073 没有改过它）：`createSession`
不收 chatid、`main.ts` 每次开页都建**全新会话**、session id 不落 localStorage——它永远
走「新建」那一支，撞不上这条拒绝。这条契约的读者是**复用 chatid 的宿主**，也就是 Java
网关（`examples/java-gateway`），完整版写在 [INTEGRATION.md](../INTEGRATION.md)
§三「安全点三」——网关作者会读那一份。

将来前端若要接「回到上次的会话」（chatid 落 localStorage），**那一刻**就得按上面这条
先查再建：第一次带声明建、之后每次不带。恢复出来的会话会带回**它当初那份**工具表，
即使前端这边的 `webCapabilities()` 已经变了——这是刻意的（历史对话就该跟历史一致）。
