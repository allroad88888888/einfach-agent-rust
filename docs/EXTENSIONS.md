# Rust 扩展包：一个第三方能带进会话的东西，以及它够不着的东西

接缝定义文档（M16）。管「**别人写的 Rust 代码怎么给这个 agent 加能力**」这一件事。

与既有接缝并列：[ADAPTER.md](ADAPTER.md)（模型差异）、[MCP.md](MCP.md)（外部进程的工具）、
[HOST-CAPABILITIES.md](HOST-CAPABILITIES.md)（宿主建会话时声明自己的能力）、
[TOOLS.md](TOOLS.md)（工具三分与命名空间）。这一份管**编译期就在同一个二进制里的那一族**。

边界由 ROADMAP §一 决策 29 拍板，不重议：**内核零脚本运行时**（不嵌 Javy /
AssemblyScript / 进程内 TS），扩展是 Rust、是**编译期依赖**；TS/浏览器生态整个长在宿主层
（web-agent 经 M10 capabilities + M14 页面工具回调）。**不做动态加载**（dylib/so）——
那是完全不同的信任模型，谁要谁单开 issue 论证。

## 一、交付物：`ExtensionPack`

一个扩展 = 一个结构体。宿主吃一包 = 一行；不装它 = 删这一行，会话逐字节回到原样。

```rust
pub struct ExtensionPack { /* 字段私有 */ }

impl ExtensionPack {
    pub fn new(name: impl Into<Arc<str>>) -> Self;
    pub fn with_tool(self, spec: ToolSpec, run: SessionToolFn) -> Self;
    pub fn with_timed(self, spec: ToolSpec, timing: CallTiming, run: TimedRun) -> Self;
    pub fn name(&self) -> &str;
}
```

包里只有两类东西，对应扩展能力面的两条路：

| 条目 | 是什么 | 谁发起调用 |
|---|---|---|
| `with_tool` | **截获式工具**：模型面的一条工具，执行体拿 `&mut Session`（[146](issues/146-intercept-registry.md) 的 `SessionToolFn`） | 模型自主调 |
| `with_timed` | **timed 工具**：开局 / 每轮收尾自动跑的钩子（[133](issues/133-call-timing-field.md) 的 `CallTiming`） | runtime，按时机 |

**纯 IO 工具（不碰 `Session` 的那种）也走 `with_tool`**：签名一致、少一套机制，闭包不读
`session` 参数就是了。为「不碰状态的工具」另开一条 executor 注册路，换来的只是多一个
必须回答的问题（同一个名字两条注册路时谁赢）。

### 二元成对，没有半开的中间态

`with_tool` 一次收 **spec + 执行体**，`with_timed` 一次收 **spec + 时机 + 执行体**。
「先声明、以后再补执行体」在类型里表达不出来。这是 [147](issues/147-migrate-intercepts.md)
的教训写进接缝：内核自己那四条手工截获的声明（`ToolTable::with_*`）和执行路径
（`dispatch.rs` 的 `if` 链）分住两个文件，改一半而另一半没跟上**不报错**——只会让模型看见
一个永远落 `unknown_tool` 的名字，或者让一个没人声明的名字被偷偷执行。

### 可逆性：**交一个函数，不填一个枚举**（决策 34 / [201](issues/201-runtime-undo-fn-delivery.md)）

`with_tool` 曾有第二个位置参数 `Reversibility`（「少给一个不编译」）。它**已经删掉**——
标签是我们在给一件自己看不见的事做分类，可以吹；而 `/undo` 需要的是一个能被调用的东西。
现在依据是执行体的返回值：

```rust
pub type SessionToolFn = Box<
    dyn Fn(&mut Session, &AgentId, &Value) -> Result<(Arc<str>, Aftermath), Arc<str>>
        + Send + Sync>;

pub enum Aftermath {
    Nothing,           // 没碰外部世界 —— 状态回滚就够了
    Undo(UndoFn),      // 碰了，这是还原它的函数
    Irreversible,      // 碰了，还不回去 —— `/undo` 撞上它停下来问
}

pub type UndoFn = Box<dyn FnOnce() -> Result<(), Arc<str>> + Send + Sync>;
```

三态**不是 `Option<UndoFn>`**：`Option` 会把「没碰」和「碰了但撤不回」压成同一个 `None`，
而落盘那一位（`agent_core::Undoability`）是三态，返回类型必须与它同构。

**白拿一档粒度**：可逆性从此是**每次调用**的属性。同一个 `fs/write` 建新文件（还原 = 删掉）、
覆盖旧文件（还原 = 写回旧内容）、写失败（还原 = 空），三次调用三种交代，枚举表达不了。

写 `UndoFn` 的三条纪律：

- **捕获执行时的现场**（旧文件内容、创建出来的资源 id）——逆是在执行的那个状态上选的。
- **不能捕获 `&Session`**（类型上也不允许：`UndoFn` 是 `'static`）。它跑在 undo 路上，那时
  core 正在回滚状态；让它同时写状态就是在一次回滚中间插一次前向写入，红线 6 的账当场乱。
  状态那半边由 journal 的回滚承担，还原函数**只管外部世界**。
- **`FnOnce`：只跑一次。** 跑挂了就是 `UndoReport::Blocked { cause: HookFailed }`，用户
  `/undo!` 越过；不会有人替他偷偷重试。

判断的责任一点没减，落点变了：**拿不准就别交函数**（等价于以前的「拿不准就 `Irreversible`」），
判错的代价一样不对称——判宽了只是多问用户一句，判窄了是真的放过一次删除。

**还原函数不跨进程**（决策 34 §九）：它是闭包，活在 runtime 的钩子表里（按 `Entry::seq` 键），
崩溃恢复之后表是空的。日志里那条 entry 仍然记着「这一步交过函数」，于是恢复后 undo 它会得到
`Blocked { cause: HookLost }` 而不是静默放过。**状态的逆跨进程有效，外部世界的逆不跨进程**——
这是边界，不是缺陷。

顺带一条如实记着的副作用：CLI/Web 上**给人看的**那个 `Reversibility` 标签，对 `ext:` 工具
从此一律显示名字规则的保守值 `irreversible`；它不再是任何行为的依据（决策 34 §八）。

## 二、命名：`ext:<pack>/<tool>` 强制

装配期硬闸，不是命名建议。裸名、`srv:`/`web:`/`desk:`/`mcp:`、别的包的 `ext:` 命名空间，
一律拒绝。

- **debug 构建**：`debug_assert!` 当场炸，文案点名是哪个包的哪个名字。
- **release**：**丢这一条**（不是丢整包），同包里合法的条目照常装。丢整包会让一个钩子名的
  笔误顺手关掉同包里那个完全合法的读工具，故障点离肇事那一行更远；丢一条则与
  `ToolTable::push_spec` / `with_timed` 逐字同一句话，读者不用记第二套规矩。被丢的那条从此
  既不进 prompt 也不进任何执行路径，安全上与丢整包等价。

包名本身不能为空、不能含 `:` 或 `/`（否则 `ext:a/b` 会同时能被包 `a/b` 和包 `a` 声称）。
**绝不 sanitize**——TOOLS.md §撞名那条：悄悄把名字洗一遍，两个本来不同的声明就撞成同一个。

**为什么强制**：069 的红利照抄 MCP——**能靠命名让撞名不可能发生，就不要去写策略**。
包名进名字之后，扩展之间、扩展与内置五档之间、扩展与 M10 注入的 `web:`/`desk:` 之间结构上
撞不了，于是不需要任何仲裁策略。

**冒用前缀比裸名更该拦**：`location_of` 从前缀推位置。一个叫 `web:foo/bar` 的扩展工具会被
判成远端，dispatch 于是登记一个等宿主回传的槽——而没有任何人会认领它。不报错、不告警，
只是这个工具永远调不通（TOOLS.md §命名空间点过的同一类静默失效）。

`location_of` 对 `ext:` 落 **`Server`**：扩展是编译期依赖，执行体就是本进程里的一个闭包，
没有任何远端回传。兜底分支本来也是 `Server`，但那一条**显式**写在
`tool_table_names.rs` 里——这是接缝的承诺，不是兜底捡到的副产品。

## 三、装配：天然两阶段，接缝承认它

一个包里的两类东西住在两个**不同时刻才存在**的容器里：

| 半边 | 装进哪儿 | 那一刻宿主手上有什么 |
|---|---|---|
| specs + timed | `ToolTable` | 只有表——`RunnerCtx::new` 还没调 |
| 截获执行体 | `RunnerCtx` | ctx 已经建好，而它**吃掉了**那张表 |

这不是设计得不好，是既有结构的事实：`RunnerCtx::new` 按值收 `ToolTable`，而截获注册表住在
`RunnerCtx` 上。所以没有「一次调用吃一包」的写法，只有「一包拆两次装」：

```rust
let (tools, pending) = ToolTable::builtin().with_shell().with_extension(demo_pack());
let mut ctx = RunnerCtx::new(/* … */, tools, /* … */);
pending.install(&mut ctx);          // ← 忘了这一行，debug 构建当场炸
```

### 防呆：「装了表半边、忘了 ctx 半边」

半开的后果是静默的：specs 进了 prompt，模型看得见这个工具、调它，dispatch 查截获表查不到 →
落进常规 `ExecuteTool` 路 → `unknown_tool`。**不报错，只是这个扩展永远不工作。**

三道锁，从「编译器能说的」到「一定会说的」：

1. **`PendingInterceptors` 不是 `Clone`、没有公开构造器**——它只能由 `with_extension` 从
   **同一个包实例**拆出来的执行体造出来（`into_parts` 消费自身）。「表装 A 包、ctx 装 B 包」
   不是「要小心的事」，是**写不出来的事**。
2. **`#[must_use]`**：返回值被整个丢掉时编译器就警告。
3. **析构炸弹**：绑了变量却从没 `install` 的那一半，`Drop` 里 `debug_assert!` 点名炸；
   release 落一条 `tracing::error!`。正在 unwind 时不炸（`thread::panicking()` 先挡）——
   drop 里再 panic 会直接 `abort`，把一次看得见的失败换成一个没有栈的进程死亡。

Rust 没有线性类型，做不到「不装就不编译」；`Drop` 是「值一定会被处理」这句话唯一的落点。
所以这里选的是**一定会说话的运行期锁**，而不是自我感觉良好的编译期锁。

**空包也必须装**：只带 timed 钩子的包拆出来的 `PendingInterceptors` 是空的，丢了也无害——
但它照样炸。纪律要对宿主统一：宿主不该知道某个包里有没有截获工具，今天空、下个版本加了
一条截获的包，会让一个「反正是空的就没写 install」的宿主在**升级依赖那一刻**静默半开。

### 顺序（红线 11）

包内条目顺序 = 源码写死的 push 顺序，**不排序**（`with_host_tools` 排序是因为客户端给的数组
顺序不可靠，包不是这样）。装配顺序 = 宿主给包的顺序。整段追加在**表尾**：前面那段所有会话
共有的字节一个都不动，前缀缓存不因为装了一个扩展而整体作废。

## 四、正门：`Session` 手套的能与不能

截获式工具是扩展访问状态的**唯一**正门。它拿到的是收窄过的公开签名（照抄
`crates/agent-runtime/src/session_tool_ext.rs:93-95` 的当前定义，别凭记忆写）：

```rust
pub type SessionToolFn = Box<
    dyn Fn(&mut Session, &AgentId, &Value) -> Result<(Arc<str>, Aftermath), Arc<str>> + Send + Sync,
>;
```

**能**：
- 读整棵 `Session::agent_tree()` 与各类型化读口；
- 写——但只能经 `Session` 的公开命令方法（`set_max_turns`/`mark_no_undo`/`spawn_child`/
  `replace_send_plan`/……），它们内部都经 `commit`/`commit_as` 落一条 journaled `Entry`；
- 返回 `Ok((正文, Aftermath))`——正文是给模型看的 tool_result，`Aftermath`（§一）如实
  交代这次调用在外部世界留下了什么；`Err` = 拒绝文案（决策 20：不 panic、不卡这一轮，
  让模型自己收敛），按「什么都没碰」记账，不进 `Aftermath` 的三选一。
  **`Err` 的语义是「没碰」，不是「失败」**——碰了一半才失败的调用要返回
  `Ok((失败说明, Aftermath::Irreversible))`，或者交回一个只收拾做了那一半的
  `Aftermath::Undo`。用 `Err` 报这种失败，等于告诉账本「外部世界干净」，
  而它不干净（`session_tool_ext.rs` 模块文档同款警告）。

**不能**：
- 碰 `Subtree`/`CompactSlots`/`IoBus`（内部层才有，扩展签名上够不着）；
- 绕过 command 层直接写 store（[红线 2](INVARIANTS.md)）——`Session` 没有暴露那个口；
- 产出 effect、起异步、等远端回写：这条路是**当场算完当场回**（无 Pending、无在飞凭据）。
  要异步就是另一条路（MCP 或宿主侧远端工具），不是这条签名的隐藏能力；
- **还原函数捕获 `&Session`**（`UndoFn` 是 `'static` 且 `FnOnce`，类型上就装不下一个借用）。
  它跑在 undo 路上，那时 core 正在回滚状态；让它同时写状态就是在一次回滚中间插一次
  前向写入，红线 6 的账当场乱。状态那半边由 journal 的回滚承担，还原函数**只管外部
  世界**（201 §注意）；
- **timed 钩子没有还原函数**：`TimedRun` 的副作用本来就不进 command log（决策 30 /
  153，`turn_end.rs` §审计面），没有对应的 `Entry` 可挂——这不是漏了一刀，是另一件事。

**纪律（机制不强制，如实写）**：

- **按调用者的后代收窄**（[红线 10](INVARIANTS.md)）：`agent_tree()` 给的是权威的整棵树，
  扩展要自己按 `agent` 参数过滤到「调用者能看到的那一段」（照 `status_tool::observe` 的
  先例，`AgentId::is_descendant_of`）。把整棵树喂给模型 = 把红线 10 挡的横读后门直接开在
  扩展层，而且**不会有任何东西报错**。
- **进 prompt 的东西逐字节确定**（[红线 11](INVARIANTS.md)）：工具描述、返回正文里别用
  `HashMap`/`HashSet` 迭代顺序，别塞时间戳/随机数——每一轮都全价重算前缀。
- **timed 钩子的账**：`TurnEnd` 钩子的副作用**不进 command log**（结果丢弃、失败只记日志，
  见 `turn_end.rs` 模块文档「审计面」）。它从不在模型的操作面上，是部署者显式装的钩子，
  不是运行时替他做的隐藏选择——但你要知道这一条。

## 五、写你的第一个扩展包（教材：`ext:stats`）

仓里有一个真跑过的样板：`crates/agent-cli/src/ext_stats.rs`（包 + 装配）与
`ext_stats_report.rs`（正文渲染）。[149](issues/149-extension-dogfood.md) 拿它做了完整的真机
dogfood——下面每个数字都是那次跑出来的，不是设想。

它带两条，正好一条一路：

| 条目 | 谁发起 | 干什么 |
|---|---|---|
| `ext:stats/report`（截获式，交 `Aftermath::Nothing`） | 模型自主调 | 读账本，回一段「这个会话至今干了什么」 |
| `ext:stats/audit`（`TurnEnd` timed） | runtime，每个完成轮 | 往 `<session>.audit.log` 追加一行 |

### 五步

**1. 组包**（二元成对：声明与执行体一起进）：

```rust
ExtensionPack::new("stats")
    .with_tool(report_spec(), report_run())
    .with_timed(audit_spec(), CallTiming::TurnEnd, audit_run(ledger))
```

**2. 执行体只做纯函数能做的事**，并**如实交代自己碰了什么**。`report_run` 拿到
`&mut Session`，第一件事是把它降成 `&Session` 交给一个纯函数，然后返回
`Ok((正文, Aftermath::Nothing))`——签名本身就是「这次调用什么都没碰」的举证的一部分。

**3. 收窄**（红线 10）：`agent_tree()` 是权威的整棵树，报告只列**调用者自己 + 它的严格
后代**。真机上一个子 agent 调它，看不到兄弟、看不到 root。

**4. 装配两行 + 一行开关**（宿主侧，`main.rs`）：

```rust
let (tool_table, ext_pending) = ext_stats::install(tool_table, ext_stats::enabled(&args), session_file.as_deref(), &mut note);
let mut ctx = RunnerCtx::new(/* … */, tool_table, /* … */);
if let Some(pending) = ext_pending { pending.install(&mut ctx); }
```

**5. 验一次「不装等于没有」**。149 的做法：把 `base_url` 指到一个本地 recorder，同一句
输入跑两个二进制（改动前 / 改动后不开开关），比 body 的 sha256——**相等**（23957 字节，
`79bd1d5c…`）。开了开关那份前 23955 字节逐字相同，多出来的一条工具整段在**表尾**。
这是「装了才有、装了只在尾部有」最省事的证据形式，比数 token 硬。

### 三条纪律，各自的真机教训

**① 报告里的每个数字都必须是状态的函数。** 自检两问：**撤销之后会回退吗？崩溃恢复之后
还在吗？** 两个都答「是」才有资格进 tool_result。

- 回退：真机两次 `/undo` 之后，同一个工具报的 agent 数 2 → 1、生效 entry 19 → 11、
  `spawn_child×1` 整条从分布里消失。**扩展这一侧没有一行代码认识「撤销」**——它只是又读了
  一次 `agent_tree()` 和 `history()` 的生效段。数字取**生效段**（`cursor()`）而不是物理
  条数，这个选择就是回退能不能兑现的全部。
- 恢复：`kill -9` 之后重启再调，崩溃前的每个数字原样在（agent 数不变、`prefix_init` 仍是
  1 条、label 分布逐项对得上），差的正好是崩溃后新做的事。
- **反例**（149 差点犯的）：把 `TurnEnd` 钩子的轮计数放进正文。那是进程内存里的数，
  `kill -9` 之后归零——功能看着正常，只在崩溃恢复那一刻不一致，而且长得很像一个状态 bug。

**② 够得着不等于可以走。** `Slot::Messages` 是 Upward-only（`cross_read.rs` 的可见性表），
`read_descendant` 会拒；但扩展手里是整个 `&mut Session`，`messages_of(child)` 照样拿得到。
所以 `ext:stats/report` 只报**调用者自己**的消息条数，子 agent 只报 status 那一档
（id / 深度 / 活动 / task），跟 `status_tool::observe` 逐字同一条线。机制不拦你，纪律拦。

**③ 自己截断**（决策 19，32 KiB）：agent 列表截 20 行、task 截 60 字符、整段兜底 8 KiB。
长会话下自己收，别指望 core 兜底截得好看。

### 153（决策 30）：`TurnEnd` 钩子拿只读 `Session` 现读

hook 拿只读 `Session` 现读——`TimedRun` 自 153 起是 `Fn(&ToolTable, &Session, &Value) ->
Result<Arc<str>, Arc<str>>`，`audit` 在轮末直接用这个参数现算 `entries`/`agents`/`tools`，
不再需要经 `report` 传话、也不再需要标注「这份数字是哪一轮观测的」：

```
turn=4 entries=19/19 agents=2 tools=2
turn=5 entries=11/11 agents=1 tools=1
turn=6 entries=25/25 agents=1 tools=2
```

`&Session` 是**只读**的——类型上写不了状态，「hook 不写状态」这句 v1 边界从纪律变成了
签名本身。要给这条时机再加别的能力（回灌结果、续 loop、写状态）不是这一刀批的，见
[150](issues/150-derived-extension-decision.md)（决策 30）：状态谓词触发的反应式层
（「没人喊自己动」）连同「扩展 derived 公式」的完整讨论一起记档不建，151/152 随之撤销。

## 六、与 MCP / 宿主 capabilities 的分工

三条路都能给会话加工具，选哪条看**代码在哪、信任来自哪**：

| | **扩展包**（本文档） | **MCP**（[MCP.md](MCP.md)） | **宿主 capabilities**（[HOST-CAPABILITIES.md](HOST-CAPABILITIES.md)） |
|---|---|---|---|
| 代码在哪 | 同一个二进制，编译期依赖 | 另一个进程，JSON-RPC 往返 | 宿主侧（浏览器 / Java 网关），远端回传 |
| 谁写的 | 自家/受信的 Rust 作者 | 任意第三方 server | 客户端自己 |
| 名字 | `ext:<pack>/<tool>` | `mcp:<server>/<tool>` | `web:`/`desk:` 强制 |
| 位置 | `Server`（本进程） | `Server`（宿主本地起子进程） | `Web`/`Desktop` |
| 可逆性从哪来 | **执行体每次调用现交**（`Aftermath`，决策 34） | `readOnlyHint: true` 这个**事实**被采信、不挡；其余（含查不到、`Reversible`）一律 `Blocked`（202） | 声明只是**自我描述**，不影响行为；`pure` 这个**事实**不挡，`reversible`/`irreversible` 一律 `Blocked`（决策 34/202，见 [HOST-CAPABILITIES.md](HOST-CAPABILITIES.md) §五） |
| 何时装 | 编译期决定，装配期一行 | 握手时 `tools/list` | 每次 `POST /sessions` 的请求体 |
| **能不能碰 `Session`** | **能**（截获正门，本文档 §四） | 不能 | 不能 |
| 撞名怎么办 | 前缀强制 → 结构上撞不了；同名重复 = 后来的整条丢 | 名字自带 server id → 撞不了 | 整份 400，会话不创建 |

**选择判据**：要读写 agent 状态 → 只有扩展包；要接一个已经存在的第三方工具生态 → MCP；
能力天生跑在用户那一侧（点浏览器、问真人、读前端内存）→ 宿主 capabilities。

**runtime 不认识「扩展」一词**：装完之后，扩展的工具跟自有工具走的是同一条 dispatch、同一张
表、同一套 undo 账——跟 skills 那次一样，接缝的成功标志是内核里找不到这个词。

## 七、落地状态

- [146](issues/146-intercept-registry.md)：截获注册表（正门机制）✅
- [147](issues/147-migrate-intercepts.md)：既有四条截获迁进注册表 ✅
- [148](issues/148-extension-pack-seam.md)：本文档描述的 `ExtensionPack` 接缝 ✅
- [149](issues/149-extension-dogfood.md)：宿主接线（CLI `--ext-stats`）+ 第一个真扩展包
  `ext:stats`，**真机 dogfood 六条全过** ✅（§五就是它的教材化）
- [150](issues/150-derived-extension-decision.md)：扩展观测决策拍板（决策 30）——
  「被问才算」，不建反应式层；`TimedRun` 加 `&Session` 是唯一批的实现刀 ✅
- [153](issues/153-timed-run-session.md)：150 那把刀落地——`TimedRun`/`TimedTool::run`
  加只读 `&Session`，`ext:stats/audit` 轮末现读、149 的 `Ledger` 传话格与 `seen_at`
  整个删除 ✅（M16 终点；151/152 因决策 30 撤销，不再有产出）

`agent-runtime` 侧的落点：`extension_pack.rs`（形状与名字规则）、
`tool_table_extension.rs`（两阶段装配）、`intercept_registry.rs`（正门机制，146）。
