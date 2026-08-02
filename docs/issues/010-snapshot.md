# 010 `snapshot.rs`：快照与恢复

**里程碑** M2 · **依赖** 019 · **模型** opus · **独立测试 agent** ✅ · **状态** 完成

## 目标

快照 = 序列化所有 primitive atom。恢复 = 重建图 + 灌回 primitive + derived 自动重算。

## 关键：恢复就是 redo

载入最近快照 → 把之后的 `Entry` 按 `next` 一路往前推。**那就是 redo 的循环，
同一个函数**，不写第二套加载逻辑。

这也是红线 1（derived 必须纯函数）的根据：重放要能得出同样的结果。

## 做什么

```rust
Snapshot = Vec<(AtomKey, AgentValue)>   // 只存 primitive
```

schema 演进白拿：新增 slot 在旧快照里找不到 key → 用默认值；删掉的 slot 在快照里
是多余项 → 忽略加一条 warn。**不需要写迁移脚本。**

## 验收

- 快照存盘 → 新进程载入 → 所有 derived 重算后与存盘前逐值相等
- 往构图函数中间插一个新 atom，旧快照仍能正确恢复（新 atom 取默认值）
- 恢复路径与 redo 走同一个函数（用测试钉住，不是靠自觉）

## 注意

primitive 的值必须全部可序列化（红线 3）。活对象（`JoinHandle`、`oneshot::Sender`、
HTTP stream）放 store 外的 runtime registry，atom 里只放可序列化句柄。
`AgentValue` 因此**不提供** `Opaque(Arc<dyn Any>)` 这类变体——给了就一定有人塞，
然后快照有洞，而洞要等到第一次真的从崩溃恢复时才发现。

## 实做记录（实现 agent，2026-08-01）

### 落地的文件

| 文件 | 行数 | 职责 |
|------|------|------|
| `crates/agent-store/src/history/snapshot.rs` | 89 | 快照长什么样：`Snapshot<K, V>`，可落盘。**对 store 一无所知** |
| `crates/agent-store/src/history/capture.rs` | 228 | 采集与灌回：`Store` 与 `Snapshot` 之间的整份搬运（`capture` / `restore`） |
| `crates/agent-store/src/history/parts.rs` | 230 | `History` 的持久化边界：`to_parts` / `from_parts` / `InvalidHistory` |
| `crates/agent-store/src/history/snapshot_roundtrip.rs` | 235 | 仅测试：全链路（采集 → 存盘 → 全新构图 → restore → `apply_next`） |
| `crates/agent-store/src/history.rs` | 69（+18） | 门面：模块表四行、`pub mod snapshot` 与三个 `pub use` |
| `crates/agent-store/src/lib.rs` | 31（+3） | re-export `Snapshot` / `capture` / `restore` / `InvalidHistory` |

钉死的四个签名一字未改。**没有动 `store/` 下任何文件**，也没有动 `log.rs` / `cursor.rs` /
`record.rs` / `cap.rs` / `apply.rs` 的现有内容（`from_parts` 用的是 `History` 的
`pub(super)` 字段，新开一个 `impl` 块，不碰旧的）。

### 设计判断

1. **公开面被红线 4 的检查器劈成了两个文件，而它劈得对。** `Snapshot` 必须
   `derive(Serialize)`（钉死的签名），`capture` / `restore` 必须收 `AtomId`（进程内句柄），
   两者放一个文件里 `check-invariants.sh` 当场报红线 4 —— 那正是「快照里可能混进 `AtomId`」
   的物理条件。拆开之后「可落盘的那一侧根本没有 `AtomId` 这个符号」成了结构事实。
   这与 009 的 `log.rs` / `record.rs` 是同一刀、同一个理由。
   **代价与补偿**：issue 原文点名 `snapshot.rs` 一个文件，所以 `history::snapshot` 是
   `pub mod`，并在其中把 `capture` / `restore` 再导出一次 ——
   `history::snapshot::{Snapshot, capture, restore}`、`history::{...}`、`agent_store::{...}`
   三条路径都通，调用方不必知道这一刀切在哪。
2. **`History` 的整份存取放在新文件 `parts.rs`，不放 `log.rs` 也不放 `snapshot.rs`。**
   ① 放 `snapshot.rs` 是把两个层面糊进一个文件：快照是「primitive 的值」，日志三元组是
   「命令记录」，两者唯一的关系是恢复时配套使用，不是同一件事。② 放 `log.rs` 更像，但
   `log.rs` 的职责是「日志长什么样 + append 怎么铸 seq」，不变量在那里是**构造器与
   `append` 隐式维护**的；`from_parts` 的职责是**不信任外来数据并拒绝**，那是个校验器。
   ③ 行数上 `log.rs` 275 + 本次 ~140 = 415，顶破 300 而说不出「拆了反而更难读」。
3. **`InvalidHistory` 的校验边界就是那三条，不多查。** 三条各自对应一条**恢复之后才发作
   的静默错误**：游标越界 → 下一次 undo 下标 panic；seq 重号/倒序 → 落盘日志再也无法定位
   「这一步是哪一步」，审计回放对不上；`next_seq` 太小 → 下一次 `append` 铸一个用过的号。
   不查的两类也写进了文档：**语义**（`prev`/`next` 与当前世界是否一致）要读 store，日志侧
   对 store 一无所知；**可疑但无害**（比如 `changes` 为空的条目，`append` 永远不产出）拒了
   只会让旧版本写的日志在新版本里打不开。另外钉死的枚举只有三个变体，别的问题连编码空间
   都没有 —— 边界是签名给定的，不是我临时划的。
   两个具体裁决：**`cursor == len` 合法**（游标在栈顶是常态）；**空 `entries` 时 `next_seq`
   不设下限**（cap 把老条目全裁光之后 seq 不回收，必须留在高位，此时没有任何东西能给它
   定下界）。
4. **`cap` 与 `drop_events` 不进三元组。** `cap` 是配置不是状态（`cap.rs` 已裁决默认 `None`、
   由会话层 `set_cap`）：存进落盘件等于把「这个部署允许多长的日志」冻进历史数据，改配置
   之后旧会话还按旧上限跑。`drop_events` 是「还没被取走的账」，进程都换了，上一次的裁剪
   事件没人再需要转发。恢复出来的日志因此无 cap，会话层照常 `set_cap(Some(100))` ——
   那一下会立刻裁剪一次，正好是「载入一份比现在的上限还长的旧日志」想要的行为。
5. **`restore` 的 `resolve` 返回 `Option`，而 applier 的是 get-or-create —— 这是全仓唯一
   一处两者分岔，分岔有理由。** applier 的键来自本进程刚写出来的日志，一定属于当前 schema，
   「不在」只可能是被逐出了（019），建回来正是对的；`restore` 的键来自**上一次进程的
   schema**，可能是这版代码里已经删掉的槽位，对它 get-or-create 会凭空造出一个没人读、
   也永远不会被回收的 atom（泄漏，且状态里多出一个不属于当前 schema 的槽位）。所以这里
   必须能说「不认识」，于是 `Option` + `on_unknown`。`on_unknown` 而不是静默丢：报 warn
   是 IO，store 层不做（红线 7），只给回调。
6. **`restore` 整批包在一个 `store.batch` 里**（019 同款理由）：恢复是一次状态跃迁，不批
   就是每写一个 primitive 冲一次 flush，下游 derived 在「一半上次会话一半这次会话」的世界
   上重算若干次。自测断言了下游重算增量**恰好是 1**。
7. **`capture` 不排序、不去重、不翻 store。** 翻 store 只能拿到 `AtomId`，按 id 存盘正是
   红线 4 禁的那件事 —— 「哪些槽位属于这个会话」是 family 遍历，而键的语义（哪个 agent、
   哪个 slot）store 层不知道。排序同理：要求 `K: Ord` 就是替上层决定键怎么比较。
   **落盘字节要不要逐字节确定（红线 11 的同一个理由，`AtomFamily` 内部正是 `HashMap`）
   因此是上层的责任**，两个 roundtrip 测试里都是先 `sort` 再 `capture`。
8. **`restore` 不产出 `Change`、不碰 `History`。** 它铺的是**世界的起点**，不是世界里的
   一步。恢复之后日志的游标由 `from_parts` 给定（落盘件里快照与游标是配套的，正是
   `SessionStore::load` 的 `(Snapshot, Vec<Entry>, usize)`）。
9. **`to_parts(self)` 消费 self，没加 `#[allow(clippy::wrong_self_convention)]`。** 先按
   钉死签名写，再实测 clippy `-D warnings` 不报（泛型 self 类型这条 lint 没触发），于是
   不留无用的 allow。消费而不是克隆是刻意的：三元组就是把内部字段整个搬出去。

### 「恢复就是 redo」是怎么被钉住的

不是靠自觉，是靠 `snapshot_roundtrip.rs` 里两条路殊途同归：

- `direct`：`restore(快照)` → **字面调用 `apply_next(&store, &mut resolve, &entries[2..])`**。
- `via_redo`：`restore(快照)` → `History::from_parts(entries, cursor = 2 /*快照点*/, next_seq)`
  → `while log.can_redo() { apply_next(…, log.redo_one() 的产物) }`。

两边与原世界（`world_a` 第 4 步之后）**逐值相等**，`log.cursor()` 落回 4，再 append 铸的是
seq 4（续铸不重号）。「快照点之后的重放」和「用户按 redo」在代码里是同一个 `apply_next`，
不是复制它的循环 —— 换掉其中任何一个，这个测试当场红。

### 自测（19 个，全部内联，没建 `tests/` 新文件）

- `snapshot.rs`（3）：serde 往返保留逻辑键与顺序、空快照往返、同键出现两次原样往返。
- `capture.rs`（6）：按喂进来的顺序读活值、空迭代器/空快照都是 no-op（且不触发重算）、
  已知键全落地且下游**恰好重算一次**、未知键进 `on_unknown` 且**没有凭空建出 atom**、
  快照里没有的键保持构图函数给的默认值、重复键后写覆盖先写。
- `parts.rs`（6）：往返恒等、三种坏输入各自被拒（并各带一个「合法的边界值」对照：
  `cursor == len`、seq 跳号、`next_seq = last + 1`）、空 `entries` 允许高位 `next_seq`、
  往返后 undo/redo/append 与原件逐步同形（含续铸不重号）、cap 裁过的会话往返后接着高位铸号。
- `snapshot_roundtrip.rs`（4）：**验收 1**（存盘 → 新 store 全新构图 → restore + `apply_next`
  → 逐值相等）、**验收 3**（恢复路径与 redo 同一个函数，见上）、**验收 2**（构图函数中间插
  `mid`，旧快照仍正确恢复：新槽位取默认值 7、旧键全灌回、`assert_ne!(slot(v1,"c"), slot(v2,"c"))`
  当场展示「存 `AtomId` 就已经错位了」）、删掉的 slot 走 `on_unknown` 且其余照常。

值类型在 `snapshot_roundtrip.rs` 里用的是 `i64`（`impl AtomValue for i64`）：它的 `Serialize`
来自 serde 自己的实现，于是这个既有 `AtomId` 又要序列化的文件**一行 `derive(…Serialize…)`
都不需要**，红线 4 的检查器在这里同样不可能被触发。

### 验收结果

- `cargo test -p agent-store`：**143 passed / 0 failed**（lib 60 = 原 41 + 本次 19；
  集成 82 含独立测试 agent 的 11 个；doctest 1）
- `cargo test --workspace --no-fail-fast`：**603 passed / 2 failed**（收工前后测了两轮，
  598→603 —— 同一个工作树里 019 那次一样的移动靶，别的 issue 在并行落地）。两个失败
  都不在本 issue 的实现侧：
  ① `agent-providers/tests/invariants_meta.rs`（全仓跑 `check-invariants.sh`）——
  见下面「异议」，卡的是独立测试 agent 的三个文件；
  ② `agent-core/tests/atom_value.rs::messages_use_the_structural_sharing_fast_path…`
  （`assert!(a.ptr_eq(&b))`，红线 5 的 `AgentValue`）—— 另一个 issue 正在并行落地的
  `agent-core`，与快照无关（本次改动只碰 `agent-store`，且 `agent-store` 全绿）
- `cargo clippy --workspace --all-targets -- -D warnings`：0 警告
- `bash scripts/check-invariants.sh <本次四个新文件 + history.rs + lib.rs>`：红线检查通过
- `bash scripts/check-invariants.sh --all`：exit 1，三处违规**全部在独立测试 agent 的
  `tests/snapshot_*.rs`**（见「异议」）
- 行数：89 / 228 / 230 / 235 / 69 / 31，全部 ≤300

### 异议：红线 4 的 grep 在 `tests/` 下对「快照」这类 feature 是不可满足的

`check-invariants.sh --all` 现在报三条违规，都在独立测试 agent 的文件：
`snapshot_roundtrip.rs` / `snapshot_recovery_is_redo.rs` / `snapshot_serde_key_is_string.rs`
——「同一文件里既有 `Serialize` 派生又出现 `AtomId`」。**他们的测试全绿，逻辑没问题**，
卡住的是那条 grep。

结构性原因：集成测试的职责恰恰是同时握着接缝两侧（要建 atom 就得写 `AtomId`，要「存盘」
就得有个可序列化的值类型）。而实现侧那条出路在集成测试里走不通 —— `impl AtomValue for i64`
在 tests crate 里违反孤儿规则（trait 和类型都不是本 crate 的），所以他们只能自定义值类型，
自定义就要 `derive(Serialize)`。对比：红线 2 的检查显式豁免了 `*/tests/*`，红线 9 也豁免，
红线 4 没有。

两条出路，请主会话裁决（我没有动他们的文件）：

- **测试侧适配**：把 derive `Serialize` 的值类型挪进已有的 `tests/common/mod.rs`
  （那个文件里没有 `AtomId`，天然通过），三个测试文件只 `use` 它 —— 和实现侧被迫做的拆分
  是同一个形状，009 的 `history_serde.rs`（刻意不 import `AtomId`）也是这个路子。
- **检查器适配**：给 `check_atomid_not_serialized` 加 `*/tests/*` 豁免，与红线 2 / 9 对齐。
  理由是红线 4 管的是**落盘的数据结构**，而测试文件不是数据结构；风险是「测试里把 `AtomId`
  塞进快照」不再被 grep 拦 —— 但那种测试会当场失败（`Snapshot<AtomId, _>` 的 JSON 里存的是
  裸 u64，恢复到新图上值就是错的）。

### 推给别人的

- **恢复之后 `set_cap` 要由会话层再调一次**（判断 4）：`from_parts` 出来的日志无上限。
  这条要写进 011 `session-store` 的载入流程，漏了就是「重启之后日志不再受限」。
- **`capture` 的键序由上层定**（判断 7）：agent 侧真正遍历 family 时必须先按 `AtomKey`
  排序再 `capture`，否则每次快照的字节都不一样（`AtomFamily` 内部是 `HashMap`），
  diff / 去重 / 内容寻址全部失效。归「状态搬进原子图」与 011。
- **`restore` 的 `resolve` 必须是非创建查找**（判断 5）：集成层若图省事复用 applier 那个
  get-or-create 闭包，删掉的槽位会被静默重建成一个永不回收的孤儿 atom，而且不报错。

### 合并记录（主会话）

双侧零分歧（独测 10 测试对并落实现未改一字全过，还先在 scratchpad 搭桩自验）。
三个判断全收：restore 的 resolve 用 Option 不用 get-or-create（外来 schema 的键
get-or-create 会造孤儿 atom——全仓唯一与 applier 分岔处，理由记档）；parts.rs
独立成文件（from_parts 是不信任外来数据的校验器，与 append 的构造器语义两回事）；
空 entries 时 next_seq 不设下限（cap 裁光后 seq 高位必须合法）。

裁决其上报的红线 4 tests 困境：**加 */tests/* 豁免**——集成测试天生同时握接缝
两侧，红线 4 管的是生产序列化路径，与红线 2/9 的既有豁免同源。豁免后 --all 通过。

推给 027 的三条已在实做记录：恢复后 set_cap 由会话层再调；capture 前按 AtomKey
排序（family 是 HashMap，不排快照字节不定）；restore 的 resolve 必须非创建查找。
