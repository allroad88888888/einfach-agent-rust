# 199 可逆性从「声明的标签」改成「交付的还原函数」（**决策**）

**里程碑** M19 · **依赖** — · **模型** **opus** · **独测** 决策类 · **状态** 未开始

## 缘起

用户 2026-08-17 读 [A Programming Paradigm for Spatiotemporal Composability][paper]
（Cordis，PKU + DeepSeek-AI）之后一句话点破：

> 不是 tool 提供的还原函数么，我们还原的时候，不就是要调用 tool 提供的这个函数，
> 没提供，就默认无法回退。

以及：

> 工作做了什么，都是工具自己知道，我们无法知道，我们也无法改回去。……
> 你弄个 agent 鬼知道能不能改回，函数的，一定能改回。

论文的时间维核心就是这个形状：effect 的类型是 `Γ → Γ × (Γ → Γ)`，
**执行的时候连同自己的逆一起返回**。我们今天是一个枚举标签。

[paper]: https://github.com/cordiverse/paper

## 现状清账（全部 grep 验证，2026-08-17）

`Reversibility` 三档今天**只有两个真实产出点**：内置 `srv:agent/spawn`
（`tool_table_names.rs:76`）与宿主声明（`host_declaration.rs:227`）。MCP 永远不产出
`Reversible`（`translate.rs`：`readOnlyHint` → `Pure`，其余 → `Irreversible`）。

**消费点全表**：

| 维度 | `Pure` | `Reversible` | 有差别吗 |
|---|---|---|---|
| `mark_irreversible` / 屏障位（`dispatch.rs:131`、`session_tool_ext.rs:82`） | 不标 | 不标 | **无** |
| `/undo` 行为 | 不挡 | 不挡 | **无** |
| 补偿动作被执行 | 无需 | **无人执行** | **无** |
| `blocks_undo()` / `is_replayable()` | — | — | **两者生产代码零调用者** |
| CLI 打印（`print/events.rs:71`）、Web 渲染（`render/tool.ts:32`） | `Pure` | `Reversible` | ✅ **仅显示** |

`is_replayable()` 没人调**是对的不是漏的**：恢复走
`agent_store::apply_next(&store, …, &in_effect)`——**重放的是 journal 里的状态值，
从不重新执行工具**，所以「仅 `Pure` 可安全重放」这条判据永远用不上。

唯一的行为测试也不区分：`subagent_parallel.rs:213` 断言「spawn 是 `Reversible`，
不该被屏障挡住」——跟 `Pure` 的行为逐字相同。

**于是今天 `Reversible` 唯一的差别是打印给人看的那个字符串，而这个字符串对两类工具
意思不同**：

- **边界内**（`srv:agent/spawn`）：补偿是 `despawn_child`，而且本仓的 undo **自动完成
  它**——子 agent 的状态就在同一个 store 里，回滚 spawn 那条 entry，子的原子跟着回滚。
  标签诚实。
- **边界外**（宿主声明的 `web:`/`desk:`）：undo 直接跳过，**没有任何人调用补偿**。

边界外那条的具体失败场景（今天就成立）：

1. 宿主声明 `{ "name": "web:crm/draft", "reversibility": "reversible" }`
2. 模型调它，在 CRM 里建了一份草稿
3. 用户 `/undo`
4. CLI 打印「回退了 3 条目」，**没有任何提示**（`undo_blocked` 只对屏障触发）
5. **草稿还在 CRM 里**

`docs/HOST-CAPABILITIES.md` §五 说「宿主说什么就按什么办」——而今天把 `reversible`
当 `pure` 办，**等于把宿主说的「发生了一件需要补偿的事」这半句话直接丢掉**。

## 根上的错误

`Reversibility` 是**我们在给一件我们看不见的事做分类**，依据只有工具名和一个宿主自己
填的字符串。标签可以吹，**函数不给就是没有**。

## 拍板

### 一、行为依据从枚举换成交付物

工具执行完，**顺手交代它在外部世界留下了什么**：

```rust
/// 一次调用在**外部世界**留下了什么，以及怎么收拾
pub enum Aftermath {
    /// 没碰外部世界 —— 状态回滚就够了
    Nothing,
    /// 碰了，这是还原它的函数
    Undo(UndoFn),
    /// 碰了，还不回去 —— 撤销撞上它停下来问（就是今天的屏障）
    Irreversible,
}

// 现在
Fn(&mut Session, &AgentId, &Value) -> Result<Arc<str>, Arc<str>>
// 改成
Fn(&mut Session, &AgentId, &Value) -> Result<(Arc<str>, Aftermath), Arc<str>>
```

**是三态不是 `Option<UndoFn>`**，这一条初稿写错过，理由值得记下来：`Option` 会把
「没碰外部世界」（`fs/read`）和「碰了但撤不回」（`shell/exec`）压成同一个 `None`——
而那正是第九条在**落盘那一位**上认定为不可接受的合并。**返回类型必须与落盘的位同构**
（`Nothing → StateOnly`、`Undo → Hooked`、`Irreversible → Blocked`，1:1），
否则装配那一步只能靠猜，而猜错的方向是「把纯读也变成屏障」或者「把不可逆的静默放过」。

`Aftermath` 是运行时词汇（住 `agent-runtime`），`Undoability` 是账本词汇（住
`agent-core`）。**两个类型不合并**：前者是工具交代的事实，后者是这条 entry 的记账，
中间那一步翻译是宿主的职责——同 `Session::mark_irreversible` 今天「宿主给结论、
core 不自己判」的既有分工。

**顺带白拿一档粒度**：可逆性从此是**每次调用**的属性，不是每个工具的属性。
`fs/write` 写新文件（还原 = 删掉）、覆盖已有文件（还原 = 写回旧内容）、
磁盘满了写失败（还原 = 空）——同一个工具三次调用三种情况，枚举表达不了，
函数天然表达了。论文 `𝔈*Γ` 那一段是同一个理由：**逆是在执行的那个状态上选的**。

### 二、还原函数住 runtime，不住 core（红线 7）

`agent-core` 不做 IO，不能持有一个会碰外部世界的闭包。所以：

- **runtime** 维护还原函数表（类比在飞 provider 凭据表、`McpRegistry` 的既有形状），
  键的选择见第九条；
- **core** 只记「这条 entry 属于哪一类」——今天那一位是 `EntryMeta.barrier`，
  第九条把它扩成三态。

**core 从头到尾不认识 `UndoFn` 这个类型**：它只在 undo 路上收一个调用方递进来的
回调，跟 `Session::mark_irreversible` 今天「宿主告诉 core 一个结论、core 不自己判」
是同一条分工（`meta.rs` 原话：**core 没有工具表，现造一个等于编造**）。

### 三、还原函数在 store 回滚**之前**跑

两个世界（store / 外部）的回滚顺序不能随便选：

| 顺序 | 还原失败时 | 判定 |
|---|---|---|
| 先跑还原函数，成功了再回滚 store | store 没动，外部没动，**一致** | ✅ |
| 先回滚 store，再跑还原函数 | store 说没发生，CRM 说发生了 | ❌ **正是红线导言点名的静默错值** |

所以 `undo_turn` / `undo_step` 要收一个**回调**，在 `apply_prev` 之前逐条调用。
**core 仍然不持有闭包**——它只是调用方递进来的一个 `&mut dyn FnMut`，跟
`History::undo_turn(same_turn, is_barrier)` 今天已经收谓词参数是同一个形状，不是新发明。

### 四、逆序调用——这条是白拿的

论文 Theorem 16 证了**按逆序（LIFO）撤销不需要任何前提**（任意顺序才需要 effects
两两独立，Corollary 21）。我们的 journal 本来就是逆序走的，所以这一条不需要任何工作，
**但要在实现里写死，不许「顺手优化成并行」**。

### 五、还原失败 = 复用屏障，不新开机制

用户拍板的语义：

> 状态就停在那一步，用户可以强制往回退，就等于跳过这一步。

**这逐字是现有的屏障语义**：`UndoReport::Blocked { entries, barrier_seq }` →
用户确认 → `undo_turn_force()` 跳过这一条接着退。

**明确否决**：给 undo 加一个 `ignore_undo_errors: bool` 参数。它是**事先设一次、
替用户答了所有他还没被问到的问题**，正是 `undo.rs` 两处注释拒绝过的形状——

> `History` **不记「这条已经问过了」**：越过永远是上层的一次显式决定，不会因为某个
> 状态位而在下一次 undo 里被静默继承。
>
> 「第一条」不是「全部」：一次确认只放行一条……放行全部等于让一次确认替用户答了
> 几个他没被问到的问题。

现在这条路是**失败一次，问一次，答一次**。保持。

**但要加一处区分**：屏障是「没碰」（不知道怎么撤，停在它前面），还原失败是
「碰了，而且可能做了一半」。用户决定要不要强制越过时，这两种要看到不同的话术。
所以 `UndoReport::Blocked` 上加一个成因字段（外加错误文案），**不是加开关**。

**失败那条 entry 的状态不回滚**（同表二）。

### 六、`srv:agent/spawn` 交什么

交 `Some(no-op)`。理由：spawn 的还原（子 agent 的状态消失）**由 store 回滚本身完成**，
不需要额外动作；但它确实写了状态，所以不是「无需还原」而是「还原已由机制承担」。
交 `None` 会让每次 spawn 都变成屏障——`tool_table_names.rs` 的注释早就论证过那样错：

> 那样反而会让「拆了任务的那一轮」一律撤不掉，哪怕子 agent 只读了两个文件。

### 七、宿主工具与 MCP 一律 `None`

- **MCP**：MCP 协议里**根本没有「撤销」这个概念**，server 不会交函数。恒 `None`。
  这跟今天 `readOnlyHint` 缺失落 `Irreversible` 的默认一致，不算倒退。
- **宿主 `web:` / `desk:`**：还原函数在浏览器/桌面那边，交不回来。恒 `None`。
  `capabilities.tools[].reversibility: "reversible"` 从此**不再产生「不挡 undo」的效果**。

宿主侧的还原回调是**第二步**（往宿主推「请还原」事件、等回传、`undo_turn()` 从同步变
异步、还原失败怎么收场），**本里程碑不做**，等有真实宿主要它再开。

### 八、`Reversibility` 枚举的去留

**留，但降级成纯显示标签**，不再是任何行为的依据：

- 协议面 `CapabilityReversibility`（TS 已生成）不动——删它是破坏性协议变更，而它作为
  「宿主对这个工具的自我描述」仍然有信息量；
- CLI / Web 继续打印它；
- **但装配期要保证它不撒谎**：宿主声明 `reversible` 而交不出函数时，显示上要么降级成
  `irreversible`，要么显式标注「声明可逆但本仓不代为补偿」——具体形状在 [202](202-host-mcp-undo-none.md) 定。

`is_replayable()` **删**（见现状清账：恢复不重放工具，判据永远用不上）。

### 九、还原函数**不跨进程**——`barrier: bool` 要变成三态

写 200 时浮出来的一条，必须在这里结账。

还原函数是**闭包，活在进程里**，崩溃恢复之后表是空的。而 `EntryMeta` 落盘的
`barrier` 位是持久的。于是恢复之后会出现：日志说「这条不挡」（因为当初交了函数），
而函数已经没了——**照 `barrier: false` 走就是静默跳过一次真实副作用**。

`barrier: bool` 两态不够用，因为「不挡」实际上是两件事：这一步压根没碰外部世界
（`user_input` / `provider_done` 这类），和这一步碰了但交了函数。改成三态：

```rust
pub enum Undoability {
    /// 没碰外部世界——状态回滚就够了（今天绝大多数 entry）
    StateOnly,
    /// 碰了，且交了还原函数（钩子表按 `Entry::seq` 查）
    Hooked,
    /// 碰了，没交还原函数——屏障
    Blocked,
}
```

- `is_barrier(meta)` 变成 `matches!(meta.undoability, Blocked)`；
- **`Hooked` 但表里查不到 → 按还原失败处理**（`UndoReport::Blocked`，成因写明
  「还原函数已随进程重启消失」）。用户照样能 `undo_turn_force()` 强制越过，
  语义与话术都跟别的失败一致；
- **老会话文件的迁移是逐字确定的**：`barrier: true → Blocked`、`barrier: false →
  StateOnly`。老会话本来就没有钩子，这个映射对它们是**真的**，不是将就。

**钩子表按 `Entry::seq` 键，不按 `ToolCallId`。** `EntryMeta` 今天没有 `call_id`
（只有 `turn_id`/`epoch`/`label`/`barrier`），加一个就要动落盘 schema；而 `seq` 由
`History` 铸造、严格递增、本来就在 `Entry` 上，runtime 提交完读一次 `last_entry()`
就拿到了。**能不加字段就不加。**

这条同时把一件事记清楚：**状态的逆跨进程有效（journal 的 prev/next 是数据），
外部世界的逆不跨进程（它是闭包）**。这正是论文那套机制拿不到崩溃恢复的原因，
我们只是把边界画明白，不是退步。

### 十、明确不做

- **让模型 / 子 agent 去「想办法撤销」**。用户原话：「你弄个 agent 鬼知道能不能改回，
  函数的，一定能改回。」这是本仓最该防的一类：**函数失败会报错、会有栈、会停下来；
  模型「撤销」会给你一句「已清理完毕」然后什么都没做，而你没有任何办法知道**。
  跟红线导言那条判据同源——失败模式是「看起来成功了」。
- **宿主侧还原回调**（第二步，见七）。
- **论文的空间维**（reactive coeffects：依赖声明 + 运行期重新连线）。用户 2026-08-17
  的判断：我们的工具表由宿主一次性装配、会话开始时交给模型、**运行实例内不再变**
  （`docs/HOST-CAPABILITIES.md` §三），**有一个时刻有一个人掌握全部信息**，论文那半边
  解决的问题在我们这里从源头就不存在。运行期重新连线更是红线 11 的正对面
  （038 探针：中途改工具数组在 DeepSeek 上把缓存归零 = 120 倍的钱）。
  **这一条记在这里，是为了以后有人拿着这篇论文再来提时不用重开讨论。**

## 验收

决策类。这个 issue 完成 = 上面十条落进 `docs/ROADMAP.md` §一成为一条编号决策，
且 200–203 每条的取舍都能追溯到它。

## 注意

- 别在 200–203 里重开「要不要保留 `Reversibility` 枚举」「失败要不要加开关」
  「要不要让模型撤销」这三个话题，它们在这里结账。
- 第三条（顺序）是这次改动里唯一会**静默出错**的地方：写反了不报错、测试也未必红，
  只在「还原函数失败」那条罕见路径上浮出来。200 的验收必须有一条专门钉它。
