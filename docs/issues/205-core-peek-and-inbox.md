# 205 core：横读全开（含订阅）+ `Visibility` 收两态 + `Inbox` 槽 + 三条命令

**里程碑** M20 · **依赖** [204](204-agent-mesh-decision.md)（拍板） · **模型** **opus** · **独测** ✅ · **状态** 待做

## 目标

把决策 204 §一 / §二 的 core 半边落下：**方向约束整条去掉，换成「边只许指向
primitive」；`Visibility` 三态收成两态；多一个带送达时机标记的私有收件箱槽和三条命令。**

## 做什么

### 1. 两个新读口，都不查方向

```rust
/// 横读（响应式）：建边，目标一变调用方跟着重算。**订阅走这条。**
/// 只接受 primitive 槽位 —— 这是「环不可能」现在唯一的依据（204 §一）。
pub fn read_agent(&self, target: &AgentId, slot: Slot) -> Result<AgentValue, ReadDenied>;

/// 横读（快照）：取一次值，**不建边**。工具的一次性读走这条。
pub fn peek_agent(&self, target: &AgentId, slot: Slot) -> Result<AgentValue, ReadDenied>;
```

- **都不查方向**（这就是横读全开那一下）；
- **都查 `Private`**：`slot.visibility() == Private` → `Err(ReadDenied::NotVisible)`；
- **primitive 由类型保证，不是由检查保证**：两个口收的都是 `Slot`，`Slot` 只映射到
  `AtomKey::Agent`，只落在 source family 上（`build.rs:47`）。**在模块文档里把这条
  写死**——它是新红线 10 成立的全部理由，而它今天靠的是一个不显眼的类型事实；
- `peek_agent` **非创建**（复用 `Session::peek`）：读不到就说读不到（`NoSuchAtom`），
  不顺手在 family 里留一个没人写的 atom——`cross_read.rs` 模块文档那条理由原样适用；
- **`read_ancestor` / `read_descendant` 保留不删**，改写成 `read_agent` 加一道方向
  断言的薄封装。029 的汇聚确实是往下的，让调用点说出这个意图有价值，而且现有调用方
  与测试**一行都不用改**（照 200 保留无参 `undo_turn()` 的同一条理由）；
- 改写 `cross_read.rs` 模块文档：第一句「跨 agent 读的两个口，没有第三个（红线 10）」
  以及那张「两道校验」的表**整个过期**，按 204 §一 重写。

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

- **兄弟横读成功**：`read_agent` 与 `peek_agent` 各读一次兄弟的 `Status`，都拿到值。
- **`peek_agent` 不建边、`read_agent` 建边**：同一对 agent，`peek_agent` 之后
  reader 的依赖集合**不含** target 的 atom，`read_agent` 之后**含**。两条一起断言，
  单断一条证不出这两个口有区别。
- **无环仍然是结构保证的**（本 issue 最重要的一条，替代原来那条 `U ∩ D = ∅`）：
  断言**每一个 `Slot` 都落在 source family 上**——遍历 `Slot::ALL`，对每一个
  构造 `AtomKey::Agent(id, slot)` 并断言它在 `sources` 里、`derived` 里没有对应项。
  这是「跨 agent 的边全是长度 1 的悬边」的直接证据。加了第一个跨 agent 的 derived
  槽位时这条会红——**那正是要它红的时刻**（212 加的是新 `DerivedKey`，不是新 `Slot`，
  所以不该红）。
- 两个口读 `Private` 槽位（`TurnsUsed` / `Inbox` / `Summaries` / `Notes`）→ `NotVisible`。
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
