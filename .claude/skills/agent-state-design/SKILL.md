---
name: agent-state-design
description: 本仓新增或改动状态、工具、effect 路径、子 agent 时的判断规则。Use when 新增 atom、决定某个状态该 primitive 还是 derived、选 AgentValue 的变体或容器类型、给 ToolDescriptor 定 effect 等级、新增一条会异步回写的 effect 路径、写跨 agent 读取或汇聚多个子 agent 的 derived、决定子 agent 的 atom 何时 evict、或拆分超行数的文件。覆盖 docs/INVARIANTS.md 里无法被 grep 判定的红线 5、6、10。
---

# 状态设计判断

`scripts/check-invariants.sh` 覆盖能被 grep 判定的红线。这份覆盖需要判断的那一半。
完整规则与理由见 `docs/INVARIANTS.md`，架构背景见 `docs/STATE-MODEL.md`。

## 判断一：primitive 还是 derived

**默认 derived。** 只有回答不出「它能从别的 atom 算出来吗」时才是 primitive。

| 是 primitive 当且仅当 | 例 |
|---|---|
| 来自外部输入（用户、网络、tool 结果） | `session.messages`、`toolcall.*.result` |
| 来自用户显式配置 | `session.config`、`skills.active` |
| 是一个无法从别处推导的状态标记 | `turn.status` |

其余一律 derived。判错的代价不对称：

- 该 derived 的做成 primitive → 它进了 undo log，undo 时被回滚到一个和上游不一致的值，
  状态自相矛盾。**这是本仓最贵的 bug 形态。**
- 该 primitive 的做成 derived → 编译不过或立刻算不出来，当场发现。

所以拿不准就是 derived。

**反例**：`ui.token_estimate` 不要做成 primitive「顺手缓存一下」。它是
`f(prompt.payload)`，做成 primitive 意味着有人要手动同步它，而 undo 时不会同步。

## 判断二：值用什么容器

红线 5。`store.get()` 返回 owned 值——**每次读都 clone**；`store.set` 靠 `PartialEq`
判断变没变来决定是否传播——**每次写都比较一次**。

| 值的形态 | 用什么 |
|---|---|
| 标量（数字、bool、短枚举） | 裸值 |
| 短字符串（id、状态名） | `Arc<str>` 或裸 `String`，看是否高频读 |
| 会增长的序列（消息历史、事件流） | `im::Vector` |
| 大对象、blob、结构化 payload | `Arc<T>` |

`AgentValue` 的 `PartialEq` 第一个分支必须是 `Arc::ptr_eq` / 结构共享的指针比较。

**消息历史专门说一次**：不要 `Arc<Vec<Message>>`。append 要 `make_mut`，每次 clone
整个 Vec；一千条消息的会话每 turn append 几次就是 O(n) 反复抄。`im::Vector` 的 append
是 O(log n) 且结构共享，undo 日志里存旧版本几乎零成本。

## 判断三：tool 的 effect 等级

这个字段决定 undo 能不能越过它、崩溃恢复时能不能重发。**定错了是数据事故。**

| 等级 | 判据 |
|---|---|
| `Pure` | 重复执行任意次，外部世界不变 |
| `Reversible` | 有明确的补偿动作，且补偿本身可靠 |
| `Irreversible` | 其余全部 |

**拿不准就是 `Irreversible`。** 判错成 Pure 的代价是重复发邮件、重复扣款；判错成
Irreversible 的代价只是多问用户一次。

`Reversible` 有个额外要求：补偿动作本身必须落进 registry，不能只写在注释里。
说不出补偿动作是什么 = 它是 `Irreversible`。

MCP 来的工具：有 `annotations.readOnlyHint` 的映射成 `Pure`，其余一律 `Irreversible`。
不要猜——默认让第三方工具可重放，是把数据事故的开关交出去。

## 判断四：新的 effect 路径要有 epoch 校验

红线 6。**任何会异步回写 atom 的路径**都要过这一关：

1. 发出时捕获当前 session epoch
2. 回写前比对，不等就丢弃并取消

现有路径：`POST /tool_result`、provider 的流式回调、MCP 的异步响应。
新增任何一条同形状的路径时，检查这两步在不在。

漏了的表现是：用户在结果回来之前按 undo，一个幽灵结果写进已回滚的状态。
偶发、依赖时序、难复现。

## 判断五：跨 agent 读取与汇聚

红线 10。整棵 agent 树在一个 store 里，谁都物理可达——约束靠 API，不靠自觉。

**方向**：只有 `read_ancestor`（往上）和 `read_descendant`（往下）。想写第三个函数、
或者想直接拿 `AgentId` 去 family 里捞兄弟的 atom，停下——那条数据应该经共同祖先中转。

**可读的槽位是分方向的**，两组不相交，这是环不可能出现的根据：

| 方向 | 可读 slot |
|---|---|
| 往上（子读父） | `messages` / `config` / `skills_active` |
| 往下（父读子） | `turn_status` / `result` / `token_usage` |

想读一个不在表里的 slot 时，先问这条依赖是不是真的必要，再问它属于哪个方向。
两个方向都想读同一个 slot = 你正在造环。

**汇聚 atom 的复杂度要明确选一个，不要默认**：

- 能短路的（「有没有子 agent 还没完成」）→ 读到第一个 `Pending` 就返回，不读完
- 不能短路的（「所有子 agent 的总 token」）→ 要么接受 O(子 agent 数)，要么做成增量

写这类 derived 时在注释里写明选了哪个。一个 O(n) 的汇聚 atom，任一子 agent 一变就整体
重算，子 agent 上百时会很明显。

**evict**：子 agent 结束后可以 evict 它的 atom，但 undo / redo 的 applier 必须能对
不存在的 atom 按需重建（默认值 create，再灌 `prev`）。写 evict 之前先确认这条路径在。

## 判断六：文件怎么拆

行数由 hook 判定，**拆法**由这里判断。

按**职责**拆，不按行数凑。检验标准：能用一句不含「和 / 以及」的话说清这个文件是干嘛的。

禁止的拆法：`xxx2.rs`、`part1.rs`、往 `utils.rs` 里塞。也不要为了凑行数把强内聚的
逻辑打碎。

`agent-store` 从上游 fork 时的参考切法（原 `store.rs` 1297 行）：

- 图结构与 atom 记录
- read 求值路径
- flush + pending 调度
- 订阅分发
- debug 内省

**Rust 特有的**：`#[cfg(test)] mod tests` 内联在底部会显著抬高行数。本仓取向是把集成
测试挪到 `tests/`，源文件只留最贴身的单元测试——不要靠删测试来压行数。

## 收尾自检

改完之后过一遍：

- 新增的 atom 是 primitive 吗？它真的算不出来吗？
- 新增的 primitive 值可序列化吗？有没有藏着句柄？
- 写入走 command 层了吗？（hook 会查 `store.set`，但经由 helper 函数的绕过它查不到）
- 新增的 tool 有 effect 等级吗？拿不准填了 `Irreversible` 吗？
- 新增的异步回写路径带 epoch 了吗？
- 跨 agent 的读走 `read_ancestor` / `read_descendant` 了吗？有没有横读？
- 新写的汇聚 derived，短路还是 O(n)，注释里写明了吗？
