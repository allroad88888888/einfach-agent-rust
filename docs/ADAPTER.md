# 模型适配层怎么定义

`agent-providers` 是架构里**唯一一处「厂商差异合法存在」的地方**。定歪了差异会往两边
漏：漏进 core 就是 `match provider` 满天飞，漏进 transport 就是 HTTP 层开始懂业务。

实测差异的清单在 [probes/PROVIDERS.md](../probes/PROVIDERS.md)。这份只管**接缝长什么样**。

## 一句话与一个判据

> 适配层把「该发生什么」翻译成「这家怎么发生」。

判定任何一段代码放哪，只问一个问题：

> **它是模型相关的判断吗？**
> 是 → adapter。不是 → core。

**red line 12：core 里一条模型相关的判断都不许有。** 没有 `match provider`，
也没有 `if caps.xxx()`。下面全是这条的推论。

它同时否掉了之前那个错误：core 里的 `TurnState::build_request()` 做的不是组装是搬运
（决策 15）。**一个不做模型相关判断的「组装函数」，是接缝错位的第一个症状。**

## 三样东西过接缝

```
                  agent-core                agent-providers
                  ──────────                ───────────────
 料单 Ingredients  ────────────────────────→  组装 + 序列化
 译文 Decoded      ←────────────────────────  中立化的响应
 调整 Adjustment[] ←────────────────────────  我为这家做了哪些妥协
```

只有这三样。**没有第四样**——adapter 不回调 core，不持有 store 句柄，不发起动作。

调整在 `encode` 时就产生（降不降级组装时就知道了，不用等响应），
宿主把它随 `ProviderDone` 事件喂进 loop——core 是从事件里看到它的。

注意**没有能力位这一路**。`Capabilities` 存在，但它是 adapter 自己的配置，
不过接缝。理由见下一节。

## 料单：宁可分，不可合

料单是 core 交给 adapter 的原材料，**未加工、未合并**。

```rust
/// 纯数据且 `Send`。`Send` 不是为了跨线程，是**结构上挡住 adapter 拿到 store 句柄**
/// —— store 是 `Rc<RefCell>`，塞不进 `Send` 的类型里。
pub struct Ingredients<'a> {
    pub system: &'a [SystemChunk],  // 分段：base / skill / 动态，不预先拼成一个 String
    pub messages: &'a [Message],
    pub tools: &'a [ToolSpec],      // 开轮就在的
    pub late_tools: &'a [ToolSpec], // 本轮中途激活的 —— **不许跟上面合并**
    pub config: &'a SessionConfig,
    pub intent: RequestIntent,      // 这轮想干什么
    pub prev_prefix: Option<&'a PrefixImage>, // 上一轮的，用来算 drift 与命中预测
}
```

`tools` 是**按优先级排好的**——哪个工具对这个任务更重要是产品判断，跟模型无关。
超了某家上限时 adapter 从尾巴截断并报 `ToolsTruncated`，core 不需要知道上限是多少。

### 规则：字段怎么分，由「有没有某家会区别对待」决定

不由「逻辑上属不属于同一类」决定。

`tools` 和 `late_tools` 逻辑上都是工具。但有一家把晚加的挂到消息级、**零缓存代价**，
另两家只能并进顶层、代价 2x 到 120x。core 要是先合并成一个 `Vec<ToolSpec>`，
那家的 adapter 就再也分不出来了——**在接缝上销毁的信息不可恢复**。

同理 `system` 不预先拼成一个 `String`：前缀树匹配的那家可以把晚激活的 skill 放兄弟
分支，仅扩展匹配的那家只能追加到末尾。拼好了就没得选。

反过来：**如果没有任何一家会区别对待，就该合并。** 分开是有成本的——多一个字段就多
一处每个 adapter 都要正确处理的地方。

### 规则：料单里不许出现任何一家的 wire 字段名

所以是 `intent: RequestIntent` 不是 `tool_choice`。

`tool_choice` 是 OpenAI 系的 wire 字段名。用它当料单字段，已经假定了「翻译成
`tool_choice` 就完了」——而实际上：

| `intent` | 翻译成什么 |
|---|---|
| `Free` | 不带 `tool_choice` |
| `MustUseTool` | `tool_choice: "required"` |
| `MustUse(name)` | 有一家要**先关思考**才能传；另一家**永久不可用**，得降级 |

`MustUse` 在那家降级成什么（提示词里写死要求？改成 `required` 加校验？）是
adapter 的判断。core 只说「这轮必须调 `fs/read`」。

**命名撞上某家的 wire 字段，就是接缝错了。**

## 能力位不过接缝：从「事前问」改成「事后报」

`Capabilities` 是 adapter 内部的一张表，**`agent-core` 里 grep 不到这个词**（红线 12）。

### 为什么不让 core 读它

`if caps.can_force_specific_tool()` 看起来比 `match provider` 干净，其实是同一个病
换了层皮：

- **组合爆炸**：core 里 N 个能力位就是 2^N 条路径，其中大部分永远没被跑过
- **接缝没封住**：加第四家 provider 时要动 core，而不只是加一个 adapter
- **隐形**：事前分支静悄悄地改了行为，出问题时账单上看得见、日志里看不见

### 替代：core 说意图，adapter 报调整

```rust
pub struct Encoded {
    pub body: Vec<u8>,
    pub prefix: PrefixImage,
    /// 跟上一轮比，哪一段漂了。core 拿它做兜底第 1 层，**不需要知道为什么漂**。
    pub drift: Option<Segment>,
    /// 这次应该命中多少 token。由 adapter 按自己的匹配语义和块粒度算，
    /// core 只负责拿它跟真实 `usage` 对账（兜底第 2 层）。
    pub predicted_cache: u32,
    /// **我为这家做了哪些妥协。** 空的时候才叫「原样执行了」。
    pub adjustments: Vec<Adjustment>,
}

pub enum Adjustment {
    /// 想强制某个工具，这家做不到，降级了
    ToolChoiceDowngraded { wanted: ToolName, used: &'static str },
    /// 这家温度锁死，改了
    TemperatureOverridden { wanted: f32, used: f32 },
    /// 晚加的工具只能并进顶层，本轮前缀作废
    LateToolsForcedIntoPrefix { count: usize, est_cost_multiple: f32 },
    /// 工具数超了这家上限，按 core 给的优先级裁掉了尾巴
    ToolsTruncated { kept: usize, dropped: usize },
}
```

core 一条路径走到底，拿到结果再判断对不对。**调整进日志、进 CLI 输出、可审计**——
事前分支做不到这一点。

### 缓存兜底也照这个切

三层兜底（[024](issues/024-cache-guard.md)）里，**判断**是模型相关的，**比对**不是：

| | 谁做 | 为什么 |
|---|---|---|
| 「这次该命中多少」 | adapter | 要看匹配语义（前缀树还是仅扩展）和块粒度 |
| 「哪一段漂了」 | adapter | 它才知道自己把料摆成了什么顺序 |
| 预测 vs 真实，差太多就告警 | **core** | 纯算术，跟模型无关 |
| 滚动窗口命中率低于阈值 | **core** | 同上 |

所以 `Ingredients` 要带上一轮的 `prev_prefix`，adapter 才算得出 `drift` 和
`predicted_cache`。core 那边只有两个减法和一个窗口——**零个模型相关判断**。

### 五个能力位逐个走一遍

上一版有五个位是「core 会 `if` 的」。红线 12 之后它们全不在 core 里了——
而且每一个的替代方案都比原来**更好**，不是将就：

| 原能力位 | core 现在怎么做 | 为什么更好 |
|---|---|---|
| `should_lazy_load_tools()` | **不判断，晚加的工具一律进料单** | 原方案在贵的那家「攒着不加」——那是拿功能换钱，模型这轮压根不知道有这个工具。真实代价由 `LateToolsForcedIntoPrefix` 报出来 |
| `can_force_specific_tool()` | **不判断，永远备好降级路径** | 强制调用**在任何一家都不是保证**——支持的那家模型也可能不听话。core 本来就得校验结果，那条路必须存在 |
| `max_tools()` | **不判断，料单里 `tools` 按优先级排好** | 优先级是产品判断（哪个工具对这个任务更重要），跟模型无关。截断是 adapter 的事，报 `ToolsTruncated` |
| `compaction_cost_multiple()` | **按上下文窗口压力触发，不看折扣比** | 「贵所以晚压」听着精明，但压缩本来就该在快撑爆时做。早压是省钱优化，在最贵的那家永远不划算，在便宜的那家省得有限 |
| `siblings_share_prefix()` | **不判断** | 前缀相同就自动共享，不同就不共享，core 不需要做任何事 |

最后一行值得停一下：**它是个假接缝**——写下来时我以为它对应某个编排决策，
实际一个都没有。这正是「事前问能力」的典型产物：能力位照着**差异**列，
不是照着 **core 的分支**列，于是列出一堆没人用的。

### `Capabilities` 还在，只是不出 adapter

adapter 内部当然要知道自己的块粒度、匹配语义、usage 字段路径——那是它干活的依据。
它只是不再是一个**跨接缝的类型**。

> **判据**（[023](issues/023-three-providers.md) 的验收）：
> `Capabilities` 的每一位，至少有两家取值不同，**且 adapter 内部真的用到**。
> 两条有一条不满足就删。
>
> 必须等接了第二家才判得出——只接一家时每一位都只有一个取值。
> 这也是 023 单列不并进 022 的原因。

## 类型落在哪个 crate

判据一句话：**core 的事件与状态要携带的，必须定义在 `agent-core`**——依赖方向是
providers → core，反着引用编译不过。这正是红线 12 的结构保障：core 想读
`Capabilities` 连类型都拿不到。

| crate | 类型 | 为什么 |
|---|---|---|
| `agent-core` | `RequestIntent` | 意图是 core 定的 |
| | `Adjustment` | 随 `ProviderDone` 事件进 loop |
| | `ErrorClass` | 016 的错误分流按它转移 |
| | `PrefixImage` / `Segment` | core 存上一轮镜像、做纯算术比对——**只存只比，不判读** |
| | `StopReason` / `Usage` | 事件与兜底算术要用 |
| `agent-providers` | `Provider` trait / `Ingredients` / `Encoded` / `Decoded` / `StreamAccumulator` / `Capabilities` | 只在宿主与 adapter 之间流转，core 看不见 |

## trait 长什么样

```rust
/// 一家 provider 的适配器。**全部方法都是纯函数**——不做 IO、不重试、不读时钟。
pub trait Provider: Send + Sync {
    /// 组装 + 序列化。**唯一允许做模型相关判断的地方。**
    ///
    /// 妥协了什么要如实记进 `Encoded.adjustments`，静默妥协是本层的头号大忌。
    ///
    /// 返回的 `PrefixImage` 按段打标记，是缓存兜底第 1 层的输入——
    /// 对不上时要能说出**是哪一段漂了**，只报「前缀变了」等于没报。
    fn encode(&self, ing: &Ingredients) -> Encoded;

    /// 响应 → 中立结构。未知的 `finish_reason` 走 `StopReason::Other`，
    /// **不许猜成 `EndTurn`**——猜错了 loop 会以为轮次正常结束。
    fn decode(&self, body: &Value) -> Decoded;

    /// 流式累积器。三家的分帧差异实测下来是**数据不是行为**，
    /// 所以共享一个累积器，各家只给 usage 字段的取值路径。
    fn accumulator(&self) -> StreamAccumulator;

    /// HTTP 状态 + 响应体 → 错误分类。三家的错误码分配不一样。
    fn classify(&self, status: u16, body: &str) -> ErrorClass;
}
```

### 规则：方法数 = 三家真的不一样的**动作**数

一样的动作放共享函数，用参数区分。**如果一个「适配」只是一个常量不同，那它是数据不是方法。**

`accumulator()` 就是这条的产物。三处分帧差异（尾帧 `choices` 为空、`content: null`
表示空、每帧重复 `role`）实测下来一个健壮的累积器全能吃下，各家只差 usage 字段路径。
所以它返回一个共享类型，不是五个 trait 方法。

哪天真出现一家要不同的累积**逻辑**，再把它提成方法。反过来提早提成方法，
三家会写出三份 95% 相同的代码，然后其中一份修了 bug 另两份没修。

## 时序：encode 在哪个线程

117 把 provider 调用的载体从 `std::thread` 换成了同一条线程上的并发 future
（`agent-runtime/src/io_bus.rs` 的 `FuturesUnordered`），**没有「IO 线程」这一条
泳道了**——下面这张时序图不再分两栏：

```
泵所在的那一条线程（原「actor 线程」，持 store，!Send；117 之后 IO future 也在这条线程上被推进）
────────────────────────────────────────────────────────────────────────
core.step() → Effect::CallProvider { agent, epoch }
宿主从 store 取料 → Ingredients（纯数据）
adapter.encode(&ing) → Encoded { body, prefix }
缓存兜底第 1 层：prefix vs 上一轮的 prefix ← 花钱之前
io_task::task(..) 起飞，交给 io_bus 的 FuturesUnordered 推进
  ├─ native：底下另有一条工作线程，只把阻塞 socket 的字节读成行喂回这条线程
  │          （不做 encode / 累积 / epoch 判断，见 io_stream/native.rs）
  └─ wasm：`fetch` 走 spawn_local，同一个事件循环，一条线程都不起
adapter.accumulator() 喂行 → StreamEvent
宿主校验 epoch（红线 6）→ Event → core.step()
```

**`encode` 跑在这条唯一的线程上**——117 之后已经没有第二条线程可以把它挪过去了
（native 仅存的那条工作线程职责窄到只剩「把字节读成行」）。结论没变：第 1 层兜底
要拿新前缀镜像跟上一轮的比，而上一轮的镜像是状态，挪到别的执行体就得把状态也传
过去。

`Effect::CallProvider` 里**没有 payload**（[001](issues/001-loop-contract.md)）：
core 说「该调了」，不说「照这个调」。effect 变胖是接缝错位的另一个症状。

## 整层零 IO

`agent-providers` 不碰网络——那是 `agent-transport` 的活。这不是洁癖，是
[025](issues/025-provider-seam.md) 和 [023](issues/023-three-providers.md) 的验收
能写成「录制的 chunk 序列 + 断言」而不是「跑一遍看看」的原因。

**红线 7 只写了 `agent-core` / `agent-store`，但适配层同样成立**，
理由不同：core 是为了可穷举测试，adapter 是为了差异可回归。

adapter 明确不做的事：

| 不做 | 谁做 | 越界的症状 |
|---|---|---|
| 发 HTTP、重试、退避 | transport | adapter 依赖里出现 `ureq` |
| 决定「要不要调用」 | core | adapter 里出现「这轮跳过工具」 |
| 改变语义（偷偷塞 system prompt） | 没人做 | 命中率对不上但查不出为什么 |
| 持有 store 句柄 | — | 编译不过（`Ingredients: Send`） |

最后一条是结构挡住的，不靠自觉。

## 自查：放错地方的四个症状

| 症状 | 说明什么 | 怎么办 |
|---|---|---|
| core 里出现厂商名或 `match provider` | 差异漏上来了 | 吞回 adapter，改成事后报 `Adjustment` |
| core 里出现 `if caps.xxx()` | 同上，换了层皮（红线 12） | 同上 |
| 一个「组装函数」不做模型相关判断 | 它不是组装是搬运 | 挪进 adapter |
| adapter 把料单里某个字段拆开用 | core 合早了，信息在接缝上被销毁 | 料单里分开 |
| adapter 改了行为但 `adjustments` 是空的 | **静默妥协**，本层头号大忌 | 补上 `Adjustment` |

前三条是往 core 漏，第四条是接缝画错，第五条最难查——功能正常，
只在账单或「模型怎么没调那个工具」上浮出来。

## 落到哪几个 issue

| issue | 定这一层的哪部分 |
|---|---|
| [025](issues/025-provider-seam.md) | 料单 / `Encoded` / trait 的形状，一家对录制帧全绿 |
| [022](issues/022-first-provider.md) | transport + CLI，那家真被打通 |
| [023](issues/023-three-providers.md) | `Capabilities` 的每一位（接第二家才判得出真假接缝） |
| [024](issues/024-cache-guard.md) | `PrefixImage` 的分段，三层兜底 |
| [001](issues/001-loop-contract.md) | `Effect::CallProvider` 保持薄 |
