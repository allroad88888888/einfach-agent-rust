# 205 core：横读全开（含订阅）+ `Visibility` 收两态 + `Inbox` 槽 + 三条命令

**里程碑** M20 · **依赖** [204](204-agent-mesh-decision.md)（拍板） · **模型** **opus** · **独测** ✅ · **状态** ✅ 完成（2026-08-18，见文末）

## 目标

把决策 204 §一 / §二 的 core 半边落下：**方向约束整条去掉，换成「边只许指向
primitive」；`Visibility` 三态收成两态；多一个带送达时机标记的私有收件箱槽和三条命令。**

## 做什么

> **开工时的补正**（204 §一 末节）：这份 issue 初稿写的是「两个新读口，一个建边一个
> 不建边」。查错了——`read_ancestor`/`read_descendant` 走 `Session::peek` →
> `store.get(id)`（`tree.rs:126`）是**命令层的非追踪读，本来就不建边**；建边只发生在
> derived 的 read fn 里调 `args.get`，而那在生产代码里只有 `build.rs:103` 一处、
> 读的是自己 agent 的槽位。**全系统今天没有一条跨 agent 的依赖边。**
> 所以本 issue 只有**一个**新读口，而「边只许指向 primitive」的落地与测试挪去
> [212](212-await-tool-and-wait-graph.md)——那里才第一次真的建出跨 agent 的边。

### 1. 一个新读口，不查方向

```rust
/// 跨 agent 读：任意方向取一次值。**不建依赖边**——命令层的读本来就不建
/// （`tree.rs:126` 的 `peek` → `store.get`），这一点在红线 10 改写前后都成立。
pub fn read_agent(&self, target: &AgentId, slot: Slot) -> Result<AgentValue, ReadDenied>;
```

- **不查方向**（这就是横读全开那一下）；
- **查 `Private`**：`slot.visibility() == Private` → `Err(ReadDenied::NotVisible)`；
- **非创建**（复用 `Session::peek`）：读不到就说读不到（`NoSuchAtom`），不顺手在
  family 里留一个没人写的 atom——`cross_read.rs` 模块文档那条理由原样适用；
- **不加第二个「快照口」**。`Session` 层没有「建边的读」这回事，两个名字会是同一个
  实现，而两份实现就是两处可以判错的地方——`cross_read.rs:94` 把两个口的后半段合成
  一处，理由完全相同；
- **`read_ancestor` / `read_descendant` 保留不删**，改写成 `read_agent` 加一道方向
  断言的薄封装。它们**没有任何生产调用方**（四十处引用全在测试里），保留是为了让
  现有测试**一行都不用改**（照 200 保留无参 `undo_turn()` 的同一条理由）；
- 改写 `cross_read.rs` 模块文档：第一句「跨 agent 读的两个口，没有第三个（红线 10）」、
  那张「两道校验」的表、以及「兄弟互读在 API 面上不存在，环因此在结构上不可能」
  **整段过期**。新文档要说清两件事：**这里的读不建边**（所以跟环无关），
  **环的判据搬去了哪里**（212 的 derived）。

### 1b. `Visibility` 三态收成两态

```rust
pub enum Visibility {
    /// 别的 agent 读得到（任意方向）
    Shared,
    /// 只有自己读得到 —— 这个 agent 的内部账本
    Private,
}
```

- **穷举 match 保留**（`visibility.rs:59` 无通配）。它是红线 10 唯一能被编译器守住的
  部分，跟方向没关系，**不能跟着方向一起删**。
- `graph/visibility.rs` 的模块文档里那段 `U ∩ D = ∅ ⇒ 无环` 的证明**整段替换**成
  204 §一 的新论证（跨 agent 的边全是长度 1 的悬边）。
- 逐格重新站队。**九个原 `Upward` / `Downward` 的默认落 `Shared`，但 `Messages`
  要单独想**：它从「父读不到子的正文」变成「谁都读得到谁的正文」，一次就能把一轮成本
  翻几倍（204 §一）。**默认不给模型开这条路**——core 这一层放行，但工具层不暴露
  按槽位读 `Messages` 的入口（模型侧要正文有 `collect` 和 `send`）。
  这个分层要写进注释，否则下一个人加工具时会顺手开出去。

### 2. `Slot::Inbox`

```rust
/// 别的 agent 投进来、本 agent 还没消费的消息。
Inbox,
```

- `visibility()` 里显式站 **`Private`**（`visibility.rs:59` 是无通配穷举，不站队编译不过）。
  理由写进那条 match 分支的注释：**发得进去 ≠ 读得出来**（204 §二）。
- `slot_default.rs` 给空列表。
- 值形状 `Vec<InboxItem>`：

  ```rust
  struct InboxItem { from: AgentId, text: Arc<str>, when: Deliver }

  /// 这条什么时候被喂进 prompt（决策 204 §二，用户拍的两档）。
  enum Deliver {
      /// 收信人下一次组装 provider 请求之前 —— 加入本轮 loop
      Now,
      /// root 下一轮开始时 —— 这一轮结束之后才送达
      NextTurn,
  }
  ```

  **`Vec` 不是 `HashMap`**（红线 11：这些正文会进对方的 prompt）。
  **一个槽两个标记，不是两个槽**——落盘 / 恢复 / undo / `Private` 语义逐字相同，
  差别只有哪个定点来收（204 §二）。
- 红线 3：可序列化，进快照与日志用 `AtomKey`。

### 3. 三条命令

```rust
/// 往 `target` 的收件箱尾部追加一条。
/// `Now`      —— target 可以是任意活 agent（含兄弟）
/// `NextTurn` —— target **只能是 root**（子 agent 不跨 turn，见 204 §二）
pub fn deliver(&mut self, from: &AgentId, target: &AgentId, text: Arc<str>, when: Deliver)
    -> Result<(), DeliverDenied>;

/// 把 `agent` 收件箱里 `when == Now` 的条目按序追加进它的 `Messages` 并移除。
/// **`NextTurn` 的条目原地不动。** 没有待收的 → 什么都不做、**不落 entry**。
pub fn drain_now(&mut self, agent: &AgentId) -> usize;

/// 把 root 收件箱里 `when == NextTurn` 的条目按序追加进 `Messages` 并移除。
/// 由 `begin_turn` 之后、第一次组装请求之前调用（206）。
pub fn drain_next_turn(&mut self) -> usize;
```

- 三条都走命令层（红线 2），落 journal，`Undoability::StateOnly`（没碰外部世界）。
- `deliver` 拒的情形，**每一种都是显式变体不是 `Option`**（照 `ReadDenied` 的既有写法）：
  目标不活（`Session::is_live`）、目标是自己、空正文、**`NextTurn` 且目标不是 root**。
  最后那条是本 issue 最容易写漏的：投给一个下一轮不存在的收件箱，不拒就是静默丢。
- `drain_now` 返回搬了几条，让调用方决定要不要因此重新驱动这个 agent（206 用）。
  **core 自己不驱动任何东西**（红线 7）。
- 两个 drain 各只认自己那一档：**互不吃对方的条目**，这是两处验收里各有一条断言的原因。

## 验收

- **兄弟横读成功**：`read_agent` 读一次兄弟的 `Status`，拿到值。
- **读不建边**（这条替代原来那条 `U ∩ D = ∅`，是「横读开了但环仍不可能」现在的
  全部证据）：`read_agent` 之后，断言 reader 的依赖集合**不含** target 的 atom。
  今天它必然过——因为命令层的读本来就不建边——**它的价值是等哪天有人把这个口改成
  tracked 的时候会红**。
- `read_agent` 读 `Private` 槽位（`TurnsUsed` / `Inbox` / `Summaries` / `Notes`）
  → `NotVisible`。
- **`read_ancestor` / `read_descendant` 的既有测试一条断言都不改，全绿**——方向断言
  留在薄封装里，它们的行为逐字不变。
- `Visibility` 的分区测试换成两条：**每个槽位恰好站一边**（`Shared` ∪ `Private` =
  `Slot::ALL`，交集空），以及**两边都非空**。原来那条
  `the_three_visibilities_partition_every_slot` 与 `the_current_assignment_is_pinned`
  按新的两态重写，**站队清单照旧逐个钉死**（改任何一格都要先在这里改）。
- **`Messages` 的分层**：core 层 `read_agent(兄弟, Messages)` 放行；
  断言**工具层没有任何入口能触发它**（grep 式断言不算，要么是类型上不可达，
  要么是一条「工具表里没有按槽位读的工具」的测试）。
- `deliver(Now)` → `drain_now` → 目标的 `Messages` 尾部多了那条，收件箱空了。
- `deliver(Now)` 两条 → `drain_now` 一次搬两条，**顺序 = 投递顺序**（红线 11）。
- **两档互不相吃**（本 issue 第二硬的一条）：同一个收件箱里 `Now` 一条 + `NextTurn`
  一条 → `drain_now` 只搬走 `Now` 那条，`NextTurn` 那条**原地还在**；`drain_next_turn`
  反之。写成「一次搬空」这条必红。
- **`NextTurn` 熬过 turn 边界**：投一条 `NextTurn` → 这一轮跑完（`drain_now` 被调过
  若干次）→ 断言它还在收件箱里 → 下一轮 `begin_turn` 之后 `drain_next_turn` 才把它
  搬进 `Messages`。
- **`deliver(NextTurn, 非 root)` → 显式拒**（`DeliverDenied` 的独立变体）。这条是
  静默丢消息的唯一入口，别漏。
- **`drain_now` 不碰 `TurnsUsed`**（204 §二 那条唯一会静默出错的）：造一个
  `turns_used = 2` 的 agent，`drain_now` 之后断言仍然是 2。写成重置这条必红。
- `deliver` 给已经 despawn 的 agent → 显式拒，不静默丢。
- 一次 `deliver` + 一次 `drain_now` 之后 `/undo` 那一轮 → 收件箱与 `Messages`
  **都回到投递之前**，`Undoability::StateOnly` 不产生屏障。
- **落盘往返带时机标记**：`Now` 与 `NextTurn` 各一条的会话存盘 → 恢复 →
  收件箱内容**含 `when`** 逐字节相同。丢了标记 = 一条该等下一轮的消息当场被灌进去。
- `cargo test --workspace` 全绿 + `check-invariants --all` 过 + `build-wasm.sh` 绿。

## 注意

- **`read_agent` 是基础，`peek_agent` 不是它的一层皮。** 两者的差别就是「建不建边」
  ——那条差别在 store 那一层（tracked / untracked 读），把它们叠成一个实现等于把
  差别交给一个布尔参数和注释去守。`read_ancestor` / `read_descendant` 可以是
  `read_agent` 的薄封装（它们只多一道方向断言），这两个不行。
- **别给 `Inbox` 站 `Shared`**。一旦 `Shared`，任何人都能订阅任何人的收件箱，
  「谁给谁发过什么」变成响应式依赖——正是 204 §五 点名不做的那条。
- **横读全开之后，`Private` 是唯一的闸了。** 以前一个槽位站错方向，最多是多一条
  单向边；现在站错就是**所有人都读得到**。逐格站队时把这句话贴在眼前。
- `deliver` 的 `text` 会进对方 prompt：**不许在 core 里给它拼时间戳、序号、随机 id**
  （红线 11）。要标记来源就只用 `from` 那个路径 id。
- `cross_read.rs` 今天 111 行，改完会长不少（两个新口 + 重写的模块文档）。
  顶破 300 就拆：读口留 `cross_read.rs`，`ReadDenied` 与拒绝理由拆出去。
- **`visibility.rs` 那段无环证明是整段替换，不是修修补补。** 旧证明的每一句都建立在
  「两个方向的集合不相交」上，删掉方向之后它一句都不成立了——留半句比全删更糟。

## 实做记录（2026-08-18）

三门禁全绿：`cargo test --workspace` **2190 passed / 0 failed**（基线 2161）；
`check-invariants.sh --all` 退出码 0；红线 9 提示 **13 → 12**——**不是「与基线逐条
相同」**，是少了一条：207 拆掉了 `status_tool_tests.rs`（392 行），它本来就在基线
名单上。

### issue 原文错了两处，都在落地前被 grep 推翻

1. **「两个新读口，一个建边一个不建边」——查错了。** `read_ancestor`/`read_descendant`
   走 `Session::peek` → `store.get`（`tree.rs:126`）是**命令层的非追踪读，本来就
   不建边**；建边只发生在 derived 的 read fn 里调 `args.get`，而那在生产代码里只有
   `build.rs:103` 一处、读的是自己 agent 的槽位。**全系统在此之前一条跨 agent 的
   依赖边都没有**，那两个口还**没有任何生产调用方**（四十处引用全在测试里）。
   所以本 issue 只落了**一个**读口 `read_agent`，「边只许指向 primitive」的落地与
   测试挪去 [212](212-await-tool-and-wait-graph.md)——那里才第一次真的建出边。
   完整推导见 [204](204-agent-mesh-decision.md) §一末节。
2. **「现有测试一行都不用改」——错得不小。** 凡是断言「方向决定可读性」的测试都
   必然红，因为那正是被决策 35 删掉的性质。实际动了：`subagent_indep_visibility.rs`
   （028 的独立测试，整个前提「兄弟互读在 API 面上不存在」没了，重写）、
   `session_subagent_read_boundary.rs`、`collect_omits_child_turns.rs`，
   外加**十一条钉死 `Slot::ALL.len()` 的**（21 → 22，照 154 加 `HostPrefix` 的做法
   连注释一起改，不只改数字）。

### 一处比原设计更强的收获

`collect_omits_child_turns.rs` 原来断言「父读子的正文被 core 结构性拒绝」。
`Messages` 变 `Shared` 之后那条必然失败，换成了**更强的一条**：断言子的正文非空
（它真跑过好几轮）**且**父的历史仍是定值 8。原来只证明「core 不让父读」，
现在证明「就算读得到也没有一个字漏进父的历史」——而后者才是 `child_outcome.rs`
那条运行时侧读路真正要保证的东西。

### 编译器真的在守红线 10

加 `Slot::Inbox` 时 `session_indep_accounting.rs` 里那个无通配的 `match` 当场编译
不过。那是「新增槽位不站队就编译不过」这条纪律唯一的物理落点——`Visibility` 收成
两态之后它**更要紧了**：以前站错方向最多多一条单向边，现在站错就是所有人都读得到。

### 测试：独立 agent 写的，且注入错误验过承重

按 WORKFLOW §三 派了独立测试 agent（只给验收标准 + 公开签名 + 红线条目，
四个实现文件明确列为禁读），19 条一次全绿，按职责拆成三份。

**没有只看绿就收**，照 200 那次的规矩注入了两个错误实现：

| 注入 | 结果 |
|---|---|
| `drain` 一次搬空、不分档 | **5 条红**，含两条「互不相吃」和落盘往返 |
| 排空顺手重置 `TurnsUsed` | `drain_now_does_not_touch_turns_used` 红，报「left: 0 / 排空收件箱不是新一轮」 |

agent 自己加严了两处：`TurnsUsed` 那条不只比数值，还断言**那条 entry 的 `changes`
里根本没有这个键**；消息格式测的是性质而不是把 `[来自 root/a1] ` 前缀抄死
（抄死了改一个字就要改测试，而红线 11 要的是「确定」不是「长这样」）。

三条没能覆盖的，都成立且都不是本 issue 能解的：**「读不建边」黑盒面测不了**
（`Session` 没有拿依赖集合的口，测试代码本身也不是 derived）——真正的断言点在 212；
「工具层没有按槽位读 `Messages` 的入口」是 `agent-runtime` 的事；
**子 agent 的 `turns_used` 没有读口**，所以那条在 root 上造。

### 一次流程失手，记在这里

收工提交时用了 `git add -A`，把测试 agent 已经写在工作树里的三个文件扫进了 **207**
那个 commit，而它的说明里一个字没提。已 `reset --soft` 拆成两条。
**教训**：后台有 agent 在同一棵工作树上写文件时，`git add -A` 不安全，要按路径加。
