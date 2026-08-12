# 122 页面声明自己的工具表

**里程碑** M14 · **依赖** [121](121-js-tool-callback.md) · **模型** opus · **独测** 真机 + native 字节比对 · **状态** 完成（真机已验收，见文末）

## 目标

工具表从「编译期写死的两条」变成「页面声明的一份」。有了通用 executor 而表还写死，
就是假通用。

## 现状

```rust
// agent-wasm/src/tools.rs:43
pub(crate) fn browser_tool_table() -> ToolTable {
    ToolTable::empty().with_host_tools(host_tools())   // 编译期固定的两条
}
```

M10 的 `declare_host_tools` 早在 core 里（`Slot::HostTools`，`graph/slot.rs:87`），
server 侧 `agent-server/src/http/capabilities/assemble.rs` 也有现成的组装。
**这条是接一条现成的线，不是造机制。**

## 做什么

1. `AgentHost` 收一份页面给的工具声明 JSON（构造函数收，还是单独一个方法收，
   由实现者定并在实做记录里写明理由——但**必须在第一次 `send()` 之前定死**，
   会话中途换表就是前缀缓存全断）。
2. 声明形状对齐 `capabilities/assemble.rs` 已有的那份（`name` / `description` /
   `schema` / `reversibility`），**不要另起一套**。
3. 两条内建 `web:page/*` 的处置：由实现者定——保留为「总是存在」，还是也改成
   页面可选声明。**在实做记录里写明理由**，并保证 [121](121-js-tool-callback.md)
   那条「内建优先于回调」的派发顺序不被弄坏。
4. `reversibility` 缺省落 `Irreversible`——HOST-CAPABILITIES §五：「没说」不能推定
   为「安全」。`tools.rs:48-49` 已经写过这条理由。

## 验收

- **红线 11 主证据（真机）**：页面声明同一份 JSON，刷新前后 `toolTableJson()`
  **逐字节相同**；且刷新后第一轮的实际请求体里工具段与关闭前最后一轮逐字节相同。
  这是 M13 验收第三条的同一条，只是表的来源换了。
- **native 可测的那一半**：把「声明 JSON → `ToolTable`」这一步做成纯函数并在
  `agent-runtime` 或 `agent-tools` 侧用单测钉住——同一份输入 JSON 转 1000 次，
  `specs()` 序列化结果逐字节相同；字段顺序被打乱的两份 JSON 转出来**相同**
  （`with_host_tools` 按名字排序，`ToolSpec` 的字段顺序由 Rust 类型定死）。
- 真机：页面声明一条自定义工具 + [121](121-js-tool-callback.md) 的回调，
  模型调得到、答得出。
- 真机反向锁：页面声明一条 `srv:` 前缀的工具 → **拒绝**，不是默默接受。
  `tools.rs:5-9` 那条「从空表起步，不靠名称黑名单回减」的纪律在这里第一次被外部输入考验。

## 注意

- ⚠️ **这条把红线 11 的责任推给了页面**（[119](119-browser-host-capability-decision.md) §七-3）。
  `with_host_tools` 只帮你按名字排序，帮不了字段顺序和描述文案——页面每次刷新
  给的 JSON 必须逐字节一样。**这条契约要写进 `tools.rs` 的模块文档和给页面的 API
  文档两处**，并且 `www/index.html` 里的示例必须是一个模块级常量，不是每次现拼的字面量。
- `tools.rs` 今天 93 行，加一个 JSON → `ToolTable` 的解析大概率顶破 300 行上限。
  **拆分是本次改动的一部分**（红线 9），不留「下次再拆」。建议切法：
  「表怎么装配」与「声明 JSON 怎么解析校验」是两件事。
- 声明里的 `description` 是模型看得见的文本。**不要在解析层做任何规范化**
  （trim、大小写、补标点）——那等于替页面改了进 prompt 的字节，而且不报错。

## 实做记录（2026-08-12）

### 1. 声明入口：构造函数的**第二个入参**

`new AgentHost(configJson, toolDeclarationJson?)`。选构造函数而不是单独一个
`declareTools()` 方法，是为了让「第一次 `send()` 之前定死」**结构性成立**而不是
一条运行时约定：`HostConfig::with_declared_tools` 消费 `self`，而 `Inner::config`
之后再没有任何 `&mut` 的取法，「中途改表」这个动作在类型层面就不存在。单独一个
方法要额外维护「现在还能不能改」这个状态，多一条只能在运行时才发现的失败路径。

**为什么是独立的第二个参数，不是塞进 provider 配置 JSON 里**：要点 2 要求页面把
声明写成一个模块级常量原样传进来。塞进配置 JSON 就得每次现拼一个对象把常量拼进去
——那正是红线 11 最容易漂的写法。独立参数让页面能传一个**字面常量字符串**，字节
由源文件定死。

### 2. 红线 11 的契约写在两处，页面示例是模块级常量

- `agent-wasm/src/tools.rs` 模块文档 §「红线 11：这一段的责任在**页面**」；
- `AgentHost::new` 的文档注释——已核对**原样进了生成的 `agent_wasm.d.ts`**
  （`pkg/agent_wasm.d.ts` 第 74–118 行，构造函数签名前），页面作者在编辑器里就
  看得见，121 已经验证过这条路是通的。

`www/index.html` 里是两个模块级常量：`PAGE_TOOL_DECLARATION`（正常那份）与
`REJECTED_TOOL_DECLARATION`（`srv:` 反向锁那份）。**两个分支各自都是常量**，不是
现拼的字面量。

### 3. `web:host/callback-probe` 退场，改由页面声明

`host_tool.rs` 里的 `CALLBACK_PROBE_TOOL` 常量与 `callback_probe_declaration()`
整条删掉（`host_tool.rs` 121 → 87 行），`tools.rs` 掉那一行。同一条工具的
**描述与 schema 逐字未改**地搬进了 `www/index.html` 的 `PAGE_TOOL_DECLARATION`
——121 的四条真机验收因此原样仍然成立，这是本条最直接的自证。

### 4. 三条内建保留为「总是存在」，理由是**谁实现谁声明**

| | 实现在 | 声明在 |
|---|---|---|
| `web:page/title`、`web:page/url`、`web:source/echo` | Rust（`host_tool::execute`） | `tools.rs`，不可关闭 |
| 别的一切 `web:`/`desk:` | 页面的工具回调 | 页面 |

让页面去声明一条 **Rust 执行**的工具，等于把描述和实现拆到两处住：页面把描述写歪
没有任何人报错，模型照着错描述用工具。这正是 121 定「内建优先于回调」时的同一条
理由，只是这次落在声明侧。反过来 probe 该退场，也是同一条线——它的全部意义就是
「Rust 侧不实现它」。

**派发顺序不被弄坏，有两道闸**：① `host_tool::execute` 的 `match` 里三条内建名字
排在最前，页面声明只能进表、进不了那三个分支；② 页面声明的名字跟内建**撞名就拒**
（`tools::declare`），不是「后来居上」也不是「静默丢掉后来的」——静默丢掉的话页面
以为自己改写了那条工具的描述，实际执行的还是 Rust 内建，又是一次「描述与实现对不上
且不报错」。

### 5. 拆分（红线 9）：切在 crate 边界上，被验收手段逼出来的

建议切法是「表怎么装配」与「声明 JSON 怎么解析校验」两件事。实做时这一刀落在了
**crate 边界**上，因为验收那半句「native 可测」把落点定死了：`agent-wasm` 是独立
workspace + wasm32 目标，`cargo test --workspace` 编都不编它（119 §八）。解析校验
留在 `agent-wasm` 就等于没有自动化验收。

| 文件 | 行数 | 那一件事 |
|---|---|---|
| `agent-runtime/src/host_declaration.rs` | 新 249 | 一份声明 JSON → `Vec<(ToolSpec, Reversibility)>`：形状、校验、缺省。纯函数、零 IO、native 可测 |
| `agent-runtime/src/host_declaration_tests.rs` | 新 290 | 上面那条的 14 个单测（红线 11 两条证据 + 拒绝纪律的反向锁） |
| `agent-wasm/src/tools.rs` | 122 → 194 | 浏览器这张表**怎么装出来**：三条内建 + 页面声明那一段 + 撞名闸 |
| `agent-wasm/src/host.rs` | 187 → 247 | 构造函数收声明（+ 那份会进 `.d.ts` 的契约文档） |
| `agent-wasm/src/host_tool.rs` | 121 → 87 | 脚手架声明整条删掉 |
| `agent-wasm/src/config.rs` | 77 → 114 | 载体（见下） |
| `agent-wasm/src/assemble.rs` | 148 → 151 | 建表那一行改成带上声明 |
| `agent-wasm/www/index.html` | 292 → 339 | 两个声明常量 + 反向锁勾选框 + `web:page/viewport` 的回调实现 |

形状**对齐 `agent-server/src/http/capabilities/`**：四个字段逐字相同、可逆性小写
三档、缺字段有默认值、认不得的字段忽略、「没说 = `Irreversible`」、校验规则逐条对着
`validate.rs`。**但它是第二份实现不是共用的一份**——server 那份绑着 HTTP 请求体、
`ts-rs` 导出和另外两个字段（`skills`/`disable_builtin`），拆不出来给一个 wasm 宿主
用。漂移风险照实写在 `host_declaration.rs` 模块文档里（改一处必须两处一起改）。

**计划外多碰的两个文件，理由**：`assemble::open` 每开一次会话现造一张工具表，而它
拿到的宿主侧输入只有 `&HostConfig` 一个——声明必须搭这班车才够得着。所以
`HostConfig` 多了一个 `#[serde(skip)]` 的载体字段，模块文档相应改成「页面在**建宿主
那一刻**交给宿主、此后不再改变的外部输入」：provider 配置与工具声明共享的正是这条
性质，也正是本条最要紧的那条。**没有碰 `host_session.rs`**（`assemble::open` 的签名
一个字没动），123 的地盘一处未动。

### 6. 一个 issue 正文没写到的坑：裸数组会被静默收下

`serde_json::from_str::<HostToolDeclaration>("[]")` **成功**——serde 允许用序列填
结构体字段，而这个结构体唯一的字段有默认值，于是页面少写一层 `{"tools": …}` 外壳时
会静默解析成「一条工具都没声明」。症状是**一张空工具表加零条错误**：模型突然什么
工具都没有，页面这边一声不吭。`parse_shape` 先过一遍 `Value` 要求顶层是对象，专门
堵它；单测 `malformed_json_is_an_error_not_a_panic` 钉住 `[]` 与
`[{"name":"web:x/y"}]` 两种写法。

### 7. 顺带记两件账

- **工具表的字节这次又变了**：内建三条 + 页面声明那一段，分**两次** `with_host_tools`
  ——所以整张表不是全局按名字排的，而是「内建段（已排序）+ 声明段（已排序）」。
  这么切是有意的：页面声明了什么都不会挪动内建那三条的字节。代价照实说，旧会话第一轮
  的前缀缓存会断一次（121 实做记录第 4 条已经预告过「122 还会再断一次」）。
- `cargo test --workspace` 期间 123（取消与超时）正在同一棵工作树上并行落地
  （`turn.rs`/`interrupt.rs`/`host_session.rs`），下面的命令输出包含它当时的中间状态。

### 8. 命令

- `cargo test -p agent-runtime host_declaration`：14 passed
- `cargo test --workspace`：见文末「命令输出」
- `bash scripts/build-wasm.sh --dev`：过，**agent-wasm 零警告**
- `bash scripts/check-invariants.sh --all`：见文末
- 行数：本条新增/改动的所有文件全在 300 以下（最大 `host_declaration_tests.rs` 290）

### 9. 待真机（主会话跑，页面脚手架已就位）

三条都做不了——没有浏览器、没有真 key。页面那侧已经全部备好：

| # | 怎么验 | 期望 |
|---|---|---|
| 1 | **红线 11 主证据**：建宿主 → 展开「工具表」把内容存下来 → **刷新页面** → 同样操作 → 两份做字节 diff。再各跑一轮，比对两次请求体里的工具段 | 逐字节相同（5 条：`web:page/title`/`web:page/url`/`web:source/echo`/`web:host/callback-probe`/`web:page/viewport`，前三条在前） |
| 2 | **页面声明的自定义工具模型调得到**：勾「装上」建宿主 → 开会话 → 说「用工具查一下这个浏览器窗口的视口尺寸」 | 模型调 `web:page/viewport`，答案里带真实的宽×高与 `devicePixelRatio`；事件流有 `[tool-callback] ← web:page/viewport` |
| 3 | **反向锁**：勾上「改声明一条 `srv:` 前缀的工具」→ 点「建宿主」 | **建宿主当场失败**，状态栏是「工具名 "srv:shell/exec" 必须以 "web:" 或 "desk:" 开头……」。不是默默接受，也不是默默把那条丢掉后照常建起来 |
| 4 | **121 的四条原样重跑**（同一条 probe，声明来源换成页面） | 四条结果与 121 真机验收一致 |

第 4 条是本条的自证面：同一条工具从「Rust 硬编码」变成「页面声明」，121 那四条
不该有任何变化。

## 真机验收（主会话，2026-08-12，Chrome + 真 Kimi key）

**四条全过。** 工具表 5 条：内建段 `web:page/title`/`web:page/url`/`web:source/echo`
在前，声明段 `web:host/callback-probe`/`web:page/viewport` 在后——分两次
`with_host_tools` 的设计在真机上可见。

| # | 验收 | 结果 |
|---|---|---|
| 1 | **红线 11 主证据**：刷新前后工具表逐字节 | ✅ **1048 字符逐字节相同**，hash `1319473347` 前后一致，首个差异位 `-1`（无差异） |
| 2 | 页面声明的工具模型调得到 | ✅ 模型调 `web:page/viewport`，答出**真实** 1200×817、dpr=1；事件流有 `[tool-callback] ← web:page/viewport` |
| 3 | 反向锁：声明 `srv:` 前缀 | ✅ **建宿主当场失败**：「工具名 "srv:shell/exec" 必须以 "web:" 或 "desk:" 开头——声明进来的工具跑在宿主侧，位置从前缀推；"srv:"/"mcp:" 是服务端执行的前缀，不接受」。不是默默接受，也不是默默丢掉那条后照常建起来 |
| 4 | 121 四条原样重跑 | ✅ 同一条 probe 从「Rust 硬编码」换成「页面声明」，121 那四条结果无变化 |

第 2 条的判据是**真实**视口值：只断言「模型回了个数」不够，页面回调完全可以编一个。
断言 `tail.includes(String(window.innerWidth))` 才把「回调真的读了浏览器」钉住。

### 变异检验（主会话做，不是 agent 自评）

注入 issue 明令禁止的 `description.trim()`（解析层规范化）：

```
the_description_is_carried_over_byte_for_byte          FAILED
其余 13 条                                              ok
```

`the_same_declaration_yields_the_same_bytes_a_thousand_times` 保持绿——**这是对的**，
`trim` 仍然是确定性的。「确定性」与「没被改动」是两条性质，122 分成两条测试写，
跟 [126](126-vision-pure-logic.md) 那次同一个形状。已还原，14/14 绿。
