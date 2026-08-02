# 红线

违反下面任何一条，undo / 崩溃恢复会以**静默错值**的形式出问题——不报错、不 panic，
只是恢复出来的状态和崩溃前不一样。这是本仓最贵的一类 bug，所以它们是红线不是建议。

每条给出：规则、为什么、违反后会怎样、怎么检查。

**检查方式分两类。** 能被 grep 判定的挂在 `scripts/check-invariants.sh` 上
（Edit/Write 的 PostToolUse hook + CI）；需要判断的走 skill `agent-state-design`，
在设计 atom、定 reversibility 等级、决定值类型时读。写在文档里但没人检查的规则，半年后就是废纸。

---

## 1. derived 的 read fn 必须是纯函数

**规则**：`create_derived` 的闭包里禁止读时钟、取随机数、读全局可变量、做 IO。
输入只能来自 `ReadArgs::get` / `peek`。

**为什么**：恢复 = 从快照重放 command log，重放要能得出同样的结果。

**违反后**：undo 之后重算的派生值和原来不一样，redo 也对不上；崩溃恢复出来的会话和
崩溃前是两个东西。全程不报错。

**检查**：hook 粗筛 `agent-core/src/atoms/` 下的 `Instant::now` / `SystemTime::now` /
`rand::` / `thread_rng`。绕过粗筛的情况（比如经由一个 helper 函数）靠 review。
需要「当前时间」时，把它作为 primitive atom 写进去，由 command 层在写入时取值。

---

## 2. 业务代码禁止直接调 `store.set()`

**规则**：primitive 写入一律走 `agent-core` 的 command API。裸 `store.set` 只允许出现在
`agent-store/src/` 和 `agent-core/src/command/`。

**为什么**：undo 需要每次写入都留下 `(AtomKey, prev, next)`，而**显式声明是唯一可行解**。
自动捕获变更需要给每个被追踪的 atom 常驻订阅和基线值，成本 O(被追踪 atom 数)——本仓每个
agent 的每个槽位都是 family atom，子 agent 还是动态增长的，这个成本不成立。上游 TS 的
`createHistory` 踩过并写进了注释。

**违反后**：这次写入不进 undo log。undo 越过它时，这个 atom 停在新值上，其余全部回滚
——状态自相矛盾，而且是那种「测试全过、线上偶发」的矛盾。

**检查**：hook grep `\bstore\.set\(`（约定 store 变量就叫 `store`），白名单上述两个目录 + `*/tests/*`（测试操纵 store 是本分，红线管的是业务写入绕过 undo log）。

---

## 3. primitive atom 的值必须全部可序列化

**规则**：`AgentValue` 的每个变体都能 serde。活对象（`JoinHandle`、`oneshot::Sender`、
HTTP stream、MCP 子进程句柄）放 store 外面的 runtime registry，atom 里只放可序列化的句柄。

**为什么**：快照 = 序列化所有 primitive atom。有一个不可序列化，快照就是残的。

**违反后**：快照缺一块，恢复时那个 atom 用默认值——下游 derived 全部算错。发现时机是
第一次真的从崩溃恢复的时候。

**检查**：`AgentValue` **不提供** `Opaque(Arc<dyn Any>)` 这类变体，类型系统兜住大半。
hook 额外 grep value 定义文件里的 `dyn Any`。

---

## 4. 快照与日志落盘用 `AtomKey`，不用 `AtomId`

**规则**：`Snapshot` 和 `Entry.changes` 的键是 `AtomKey`（逻辑键）。`AtomId` 只在进程内有效。

**为什么**：`AtomId` 是自增 u64（`inner.next_id += 1`），完全依赖创建顺序。

**违反后**：任何人往构图函数中间插一行 `create_atom`，所有旧快照的值**整体错位**。
不报错——因为 `Value` 类型可能恰好兼容，只是配错了 atom。

**孪生条款（019 实测钉住）**：derived 的 read fn 里**不得捕获 `AtomId`**，一律按
逻辑键现查 family。捕获 id 的 derived 在依赖被逐出重建后当场 panic（id 单调不复用，
幸而不是静默错值）——红线 4 管落盘的键，这条管闭包里的键，同一个病。

**检查**：hook grep：同一个文件里同时出现 `AtomId` 和 `derive(...Serialize`；
`*/tests/*` 豁免（集成测试天生同时握接缝两侧，本红线管生产序列化路径——010 裁决）。
孪生条款需判断 → skill / review（闭包捕获 grep 不动）。

---

## 5. 大值必须 `Arc` 包住，`PartialEq` 走 `ptr_eq` 快路

**规则**：`AgentValue` 里任何可能超过几百字节的变体一律 `Arc`。`PartialEq` 第一分支是
`Arc::ptr_eq`。消息历史用 `imbl::Vector` 而非 `Arc<Vec>`。

**为什么**：`store.get()` 返回 owned 值，**每次读都 clone**；`store.set` 靠 `PartialEq`
判断「变没变」来决定是否传播。

**违反后**：不是正确性问题，是性能悬崖。一千条消息的会话，每次读 prompt 都深拷一遍
消息历史，每次写都深比较一遍。上游 `Array` / `Lambda` 已经是 `Arc` 的，照抄。

**检查**：需要判断 → skill `agent-state-design`。

---

## 6. 在飞的 effect 必须带 epoch，回写前校验

**规则**：effect 发出时带上当时的 session epoch；结果回写前比对，不等就丢弃并取消。
undo 时 bump epoch。

**为什么**：tool call 在飞时用户按了 undo，结果回来会写进一个已经被回滚掉的世界。

**违反后**：一个「幽灵结果」写进已回滚的状态。偶发、依赖时序、难复现。

**检查**：需要判断 → skill。review 时看每个 `POST /tool_result` 和每个 provider 回调
路径上有没有 epoch 比对。

---

## 7. `agent-core` / `agent-store` 不得做 IO

**规则**：这两个 crate 的 `Cargo.toml` 不得出现 `reqwest` / `hyper` / `axum` / `tokio`
及其生态；源码不得 `use std::fs` / `std::net` / `std::process`。

**为什么**：整个 agent loop 必须能在没有网络的情况下跑单元测试——mock provider、
mock tool executor，状态流转 / undo / 恢复全部可测。

**违反后**：这些测试变成集成测试，然后就没人写了，然后红线 1–6 就没有回归保护。

**检查**：hook 判定 Cargo.toml 依赖 + 源码 `use` 语句。

---

## 8. `bind` 地址默认 `127.0.0.1`

**规则**：默认绑 loopback，监听 `0.0.0.0` 必须显式设 `AGENT_BIND`。

**为什么**：当前完全没有鉴权，这是刻意的（企业在自己的网关加）。

**违反后**：一个能跑 shell tool 的 agent 裸露到网络上。

**检查**：hook grep `agent-server` 下硬编码的 `0.0.0.0`。

---

## 9. 文件行数：普通 ≤300，复杂 ≤500

**规则**：`wc -l` 口径。「复杂」仅限强内聚的单一算法 / 状态机 / 引擎核心，
且说得出「拆了反而更难读」的理由。说不出 = 按 300 算。

**为什么**：每个文件只负责一件事——能用一句不含「和 / 以及」的话说清它是干嘛的。

**违反后**：本次改动顶破上限，拆分就是本次改动的一部分，不留「下次再拆」。

**检查**：hook 判定。>500 阻断，300–500 提示需要理由。

**例外**：`tests/`、`benches/`、生成代码、fixture、快照。

**注意**：Rust 惯例把 `#[cfg(test)] mod tests` 内联在文件底部，会显著抬高行数。
本仓的取向是**把集成测试挪到 `tests/`**，源文件里只留最贴身的单元测试。上游
`einfach-core` 的 `store.rs` 是 1297 行（含内联测试），fork 时按职责拆成五个文件：
图结构与记录 / read 求值路径 / flush + pending 调度 / 订阅分发 / debug 内省。

---

## 10. agent 之间只允许上下读，禁止横读

**规则**：跨 agent 读取只走 `read_ancestor`（往上读 `messages` / `config` / `skills`）
和 `read_descendant`（往下读 `status` / `result` / `usage`）。不提供第三个 API，
兄弟之间要交换数据经共同祖先中转。

**为什么**：整棵 agent 树在同一个 store 里，谁都物理可达。依赖图必须靠 API 约束保持是树。
两个方向可读的 slot 集合不相交，环在结构上就不可能。

**违反后**：依赖成环。上游有 `CyclicRef` 检测和 256 深度预算，所以是运行时报错不是静默
错值——但那是兜底，不是设计。横读还会让「读所有兄弟」这种 O(n) 汇聚悄悄混进来。

**检查**：需要判断 → skill `agent-state-design`。API 只暴露两个函数本身就是主要约束。

---

## 11. 会进 prompt 的东西，序列化必须逐字节确定

**规则**：工具表、skill 列表、以及任何会被渲染进 prompt 的集合，一律用有序容器
（`BTreeMap` / `BTreeSet` / `Vec`），禁止 `HashMap` / `HashSet`。禁止把时间戳、
请求 id、随机 id 写进 system prompt。

**为什么**：前缀缓存靠**逐字节相等**。`HashMap` 的迭代顺序在 Rust 里是随机化的，
同一份工具表两次序列化可能顺序不同——顶层 `tools` 又在 prompt 最前面（三家实测确认），
于是每次请求都是全新前缀。

**违反后**：不报错、不 panic、功能完全正常，**只是每一轮都全价**。
DeepSeek v4-pro 上这是 120 倍的钱。靠账单反查，等于每次都先付一遍学费。

**检查**：hook 判定——同一文件里既有 `Serialize` 派生又出现 `HashMap<` / `HashSet<`。
运行期还有前缀镜像比对这一层在发请求前拦，见
[probes/PROVIDERS.md](../probes/PROVIDERS.md) 与
[issue 024](issues/024-cache-guard.md)。

## 12. core 里不许有任何模型相关的判断

**规则**：`agent-core` / `agent-store` 里不许出现厂商名，**也不许出现能力位分支**。
没有 `match provider`，也没有 `if caps.xxx()`。core 只有一条路径。

模型相关的判断**全部**在 `agent-providers`：core 说「这轮必须调 `fs/read`」，
adapter 决定翻译成 `tool_choice: {...}`、还是先关思考再传、还是这家根本做不到于是降级。

**为什么**：能力位分支看起来比 `match provider` 干净，其实是同一个病换了层皮——
core 里每多一位就多一条分支，N 位就是 2^N 种组合，其中大部分永远没被跑过。
加一家新 provider 时，要动的是 core 而不只是 adapter，这说明接缝根本没封住。

**替代办法：从「事前问能力」改成「事后报调整」。**

core 不问「你能不能强制指定工具」，直接说意图；adapter 尽力做，做不到就在响应里
带一条 `Adjustment`：「`MustUse(fs/read)` 被降级为 `required`」。core 一条路径走到底，
拿到结果再判断对不对——它本来就要处理「模型不听话」，因为**强制调用在任何一家
都不是保证**。

这个方向反过来还便宜了三件事：调整是**可见的**（进日志、进 CLI 输出、可审计），
而事前分支是隐形的；加 provider 不动 core；测试组合从 2^N 掉回 1。

**违反后**：不报错。功能正常，直到加第四家 provider 时发现要改 core，
或者某个能力位组合在生产上第一次被走到。

**检查**：hook 判定——`agent-core` / `agent-store` 里出现厂商名、`Capabilities`、
`caps.`，或 `Cargo.toml` 里依赖 `agent-providers`。

接缝的完整定义见 [ADAPTER.md](ADAPTER.md)。

---

## 关于这些红线本身

1–6 是这套架构成立的前提，不是编码风格。它们的共同点是：**违反后不报错**，
只在 undo 或崩溃恢复时以错值的形式浮出来，而那两条路径恰恰是最少被测到的。

所以：新增 atom、新增 tool、改动 command 层时，对着这份文档过一遍。
自动检查只覆盖能 grep 的部分，剩下的靠这份文档和 skill。
