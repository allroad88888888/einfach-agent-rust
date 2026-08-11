# 025 接缝定型：一家的 encode / decode / stream 对录制帧全绿

**里程碑** M1 · **依赖** 021 · **模型** opus · **独立测试 agent** ✅ · **状态** 完成

## 目标

把 [ADAPTER.md](../ADAPTER.md) 画的接缝落成类型，并用**录制的真实帧**证明一家能走：
零网络、零 key、`cargo test` 全绿。

## 为什么从 022 拆出来

原来的 022 横跨三个 crate + 接缝类型 + CLI。按 WORKFLOW 的粒度判据这是两个 issue：
「对录制帧全绿」和「真打通」是两个能独立验证的中间态。更要紧的是**评级掺了档**——
接缝形状是 opus 级判断（错了不红，只在账单上浮出来），写 transport 和 REPL 是
sonnet 级接线。混在一个 issue 里，就是拿 opus 的钱写 REPL。

## 做什么

**动手前把 [../ADAPTER.md](../ADAPTER.md) 整份读一遍。** 这是本 issue 最要紧的输入，
写歪了后面全歪。

### 类型（先定，测试 agent 只看这个）

按 ADAPTER.md §「类型落在哪个 crate」：

- `agent-core` 加接缝词汇：`RequestIntent` / `Adjustment` / `ErrorClass` /
  `PrefixImage` + `Segment`——core 的事件与状态要携带的必须在 core
  （依赖方向 providers → core，反着引用编译不过，这正是红线 12 的结构保障）
- 新建 `agent-providers`：`Provider` trait / `Ingredients` / `Encoded` / `Decoded` /
  `StreamAccumulator`

判断一段代码放哪，就一个问题：**它是模型相关的判断吗？**
是 → adapter。不是 → core。**core 里一条都不许有**（红线 12）。

两个具体后果：

- **料单里不许出现任何一家的 wire 字段名。** 所以是 `intent: RequestIntent`
  不是 `tool_choice`——后者是 OpenAI 系的字段名，用它已经假定了「翻译成
  `tool_choice` 就完了」。
- **adapter 做了妥协必须报出来**（`Adjustment`）。静默妥协是本层头号大忌：
  功能正常，只在账单或「模型怎么没调那个工具」上浮出来。

`Encoded`（原 `ProviderRequest`）这类「组装完能带走」的中间产物归 adapter，理由是**线程边界**
（决策 16）不是组装：store 是 `Rc<RefCell>` 不 `Send`，HTTP 在别的线程。

### 一家的实现

先做 `providers.toml` 里 `[default]` 指的那家。`encode` / `decode` / `stream` /
`errors` 四个文件（红线 9，别塞一个 `mod.rs`）。

流式累积器要吃下三处实测差异（[PROVIDERS.md](../../probes/PROVIDERS.md) §三）——
即使只接一家，这三条也直接决定累积器的形状：

1. **usage 可能在 `finish_reason` 之后另起一帧，且那帧 `choices` 为空**。
   假定每帧都有 `choices[0]` 的解码器会 panic 或悄悄丢掉 usage。
   丢了 usage，[024](024-cache-guard.md) 的三层兜底全部失明，而功能看起来一切正常。
2. **有的家用 `"content": null` 表示空**，有的直接省字段。不能用「字段存在」判断有没有内容。
3. **有的家每帧重复 `role: "assistant"`**，累积时忽略。

工具调用的 `arguments` 是分片流下来的，按 `index` **累加**不是覆盖。

## 验收

全部零网络、零 key：

- 流式累积器对**录制的 chunk 序列**（`probes/results/` 的真实形状）：
  - 尾帧 `choices` 为空且带 usage → usage 拿得到，不 panic
  - `"content": null` → 不产出空 delta
  - 重复 `role` → 不污染文本
  - `arguments` 分三片 → 拼出完整 JSON
- `encode`：同一份料两次产出**逐字节相同**（红线 11）
- `encode` 对 `intent: MustUse(name)` 在这家的翻译路径有断言
  （直接支持 / 先关思考 / 降级 + `Adjustment`，按这家的实测行为）
- 错误分类：401 / 429 / 400 / 402 / 5xx 各归到不同的 `ErrorClass`，
  **402（余额耗尽）单列**——混进限流会安静地退避到天荒地老
- **`agent-core` 里 grep 不到厂商名、`Capabilities`、`caps.`**（红线 12，脚本会查）

## 注意

- 红线 12：脚本会拦 core 里的 `if caps.xxx()`，但拦不住把判断藏进一个名字中立的
  辅助函数。独立测试 agent 只看 ADAPTER.md 和本验收，看不到实现体。
- 红线 11：这是缓存命中的前提，也是 [024](024-cache-guard.md) 第 1 层能工作的前提。

**为什么是 opus + 独立测试 agent**：接缝归属（决策 15、17）写错了不会红，
会一路错到 M1 验收才在缓存命中率上浮出来——独测能验证「encode 确定不确定」，
但验证不了「这段判断是不是本来就不该在 core」。这正是 WORKFLOW 第三档。

## 实做记录（2026-08-01）

实现（opus）与验收测试（独立 agent）并行完成。合并后 workspace 151/0 全绿。

**独测抓出两个「不报错、只在账单上浮出来」的真 bug**——这正是派独立 agent 的全部理由：

1. History 段最初序列化成 JSON 数组，末尾 `]` 让「追加一条消息」不再是字节级
   延长 → 每轮误报 drift、predicted 恒 0。改成逐条消息拼接（`prefix::concat`）。
2. 工具名转义把 `_` 也转了，`srv:get_time` 变成 `srv_3Aget_5Ftime`，日志里认不出。

**合并时裁决的一个分歧**：599 判 `Retryable` 不判 `Unknown`——seam.rs 对
`Retryable` 的契约原话是「限流、过载、5xx」，599 在段内；判 Unknown 会把 522
这类中间层码变成不重试。根因是主会话给两个 agent 的规格互相矛盾（一边写
5xx→Retryable、一边写 599→Unknown），**规格冲突要在派发前自查**。`Unknown`
改用 302 测。

**工具名字符集**：探针没测过冒号斜杠能不能过，代价不对称（被拒=400 整轮废，
转义=名字长几字节），按 `[a-zA-Z0-9_-]` 保守转义。两档（可读档保留 `_`、
严格档全转），**可逆性不靠推理靠自校验**：`to_wire` 产出前先 `from_wire` 验一遍，
对不上退严格档。

**签名的一处纯增补**：`StreamAccumulator::with_name_from_wire(fn)`——流式路径
吐出的工具名是 wire 转义名，累积器是共享的认不得各家规则，不还原 router 就
按名字找不到工具。钩子是数据不是方法，符合「差异是数据不是行为」。

**记两笔待清**：
- wire 编码取舍：历史里的 `Thinking` 块不回传（这家 reasoning 只出不进）；
  `ToolResult.is_error` 不进 wire（OpenAI 系没这字段，塞 content 前缀等于改语义）
- `Encoded.body` 恒为 `stream: true`，而 trait 里的 `decode` 吃非流式体。
  两者暂不矛盾（decode 给录制帧与非流式兜底用），但接缝上没有「要不要流」的
  开关——022/012 接线时如果只走流式，宣告出来；要双轨，加参数。
