# 009 `history.rs`：command log 与 undo/redo

**里程碑** M2 · **依赖** 008 · **模型** opus · **独立测试 agent** ✅ · **状态** 完成

## 目标

事务日志式的 undo/redo。**这是本仓的核心机制**，持久化与崩溃恢复共用它。

## 做什么

```rust
struct Entry {
    seq: u64,
    turn_id: TurnId,          // 两层粒度靠它分组。由 root agent 分配
    epoch: u64,               // 在飞 effect 的校验凭证
    owner: Option<String>,    // 租户归属，现在留着以后不用迁 schema
    agent: AgentId,           // 仅 UI 显示与审计，不参与 undo 判定
    label: &'static str,
    changes: Vec<(AtomKey, Value /*prev*/, Value /*next*/)>,
}
```

**这个 issue 只做「记录」**：command 层每次 primitive 写入都在这里留下一条。
读出来怎么用分三个 issue——[017](017-undo-redo.md) 两层粒度的 undo/redo、
[018](018-history-cap.md) cap 与分支覆盖、[019](019-applier-recreate.md) 已 evict
atom 的按需重建。分开是因为四件事能各自独立验证。

## 为什么是事务日志而不是快照式

每条 entry 自带完整逆操作，于是**可截断**（丢最老的不影响剩余回滚）、**可序列化**
（键是 `AtomKey` 不是对象引用）、**代价与状态规模脱钩**（一次 undo 是 O(本条 ops 数)）。
快照式必须回溯扫描前序历史才能找到某个 atom 的上一个值，截断即永久丢失。

## 验收

- 每次 primitive 写入都留下一条 `Entry`，`prev` / `next` 都对
- derived 的重算**不产生** `Entry`——只有源状态进日志
- `turn_id` 由 root 分配，子 agent 的 entry 继承而不新开 turn
- 日志可 serde 往返，且键是 `AtomKey`

## 注意

`changes` 落盘时的键必须是 `AtomKey` 不是 `AtomId`（红线 4）。`AtomId` 是自增 u64，
往构图函数中间插一行 `create_atom` 就会让所有旧记录**静默错位**。

## 开工前修正（主会话，2026-08-01）

1. **Entry 泛型化为 `Entry<K, V, M>`**。原文的 `turn_id: TurnId / agent: AgentId /
   epoch / owner / label` 是 agent 词汇，而 history 住在 agent-store
   （ARCHITECTURE §包结构），agent-store 不许 import agent-core——007 已经为
   同一个依赖方向把 store 泛型化，history 同理。这些字段整体成为泛型 `M`（元数据），
   由上层（未来的 command 层）填充；「turn_id 由 root 分配子 agent 继承」的验收
   随之挪到「状态搬进原子图」issue。红线 4「键用 AtomKey 不用 AtomId」在这里的
   形状是：**`K` 的语义由上层选择，history 对 `AtomId` 不可见**（它连这个类型
   都拿不到——ids.rs 不 re-export 给 history 用，测试用字符串键）。
2. **范围收紧**：本 issue 只做①日志结构（`Change<K,V>` / `Entry<K,V,M>` /
   `History`，append 铸 seq）②记录入口（`record_set`：捕获 prev → 写 store →
   落一条 change）③「derived 重算不产生 Entry」的结构性证明。游标移动是 017、
   cap 与分支覆盖是 018、快照是 010——append 在游标不在顶时怎么办也归 018，
   009 的 History 甚至可以先不带游标字段。

## 实做记录（实现 agent，2026-08-01）

### 落地的文件

| 文件 | 行数 | 职责 |
|------|------|------|
| `crates/agent-store/src/history.rs` | 32 | 模块门面：为什么是事务日志、两个子文件的分工 |
| `crates/agent-store/src/history/log.rs` | 199 | 日志结构本身：`Change` / `Entry` / `History`，`append` 铸 seq。**对 store 一无所知** |
| `crates/agent-store/src/history/record.rs` | 239 | 记录入口：把一次 store 写入变成一条 `Change`（`record_set`） |

`lib.rs` 加 `pub mod history;` 并 re-export `Change` / `Entry` / `History` / `record_set`。
`Cargo.toml` 加 `serde`（workspace，已开 derive + rc）与 dev-dep `serde_json`。
**没有动 `store/` 下任何现有文件**——`record_set` 只用到 `Store::get` 和 `Store::set`，
两个都已经是公开 API，不需要给 store 加能力。

### 设计判断

1. **为什么拆成两个文件，而不是一个 `history.rs`**。不是为了凑行数（合起来 438 行，
   顶破 300 但没到 500）。真正的理由是红线 4：`record_set` 必须收 `AtomId`（进程内
   句柄），`Change`/`Entry` 必须 derive `Serialize`（要落盘）。放一个文件里，
   `check-invariants.sh` 的红线 4 检查（同一文件里既有 `Serialize` 派生又出现
   `AtomId`）当场就报——**而它报得对**：那正是「日志里可能混进 AtomId」的物理条件。
   拆开之后「日志结构这一侧根本没有 `AtomId` 这个符号」成了结构事实，红线 4 在本 crate
   永不可能被触发，而不是靠人记得别写。职责上也正好是一句话一个：一个是「日志长什么样」，
   一个是「一次写入怎么变成一条日志」。
2. **`record_set` 值相等时不调 `store.set` 就直接返回 `None`**。对 primitive 这一跳过
   不可观测：`Inner::set_atom_state` 第一件事就是同一个 `PartialEq` 比较，相等即提前
   返回，不落 pending、不传播、不通知订阅。少走一趟 `write_atom_state` 而已。
3. **`prev` 用 `store.get(atom)` 当场读，不缓基线值**。这是红线 2 的全部理由：自动捕获
   要给每个被追踪 atom 常驻订阅 + 基线值，成本 O(被追踪 atom 数)，而本仓每个槽位都是
   family atom 且子 agent 动态增长。副作用是 primitive 首读会从 `init` 落值（vanilla
   的 `readAtom` 语义），这正是我们想要的 prev。
4. **`History` 里放了一个 `next_seq` 字段，没有从 `entries` 反推**。看起来像 YAGNI 的
   反例，但 018 的 cap 从最老一端丢条目、分支覆盖丢游标之后的条目，两种情况下
   `entries.len()` 和 `last().seq + 1` 都会让 seq 重复。seq 一重复，落盘日志就无法定位
   「这一步是哪一步」。8 个字节买这个。**游标字段确实没放**（017 加）。
5. **首条 entry 的 `seq` 是 0**。钉死的签名只要求「严格递增」，取 0 起：与 `entries()`
   的下标在未截断时天然对齐，018 截断后二者分叉，正好是「seq 不是下标」的提示。
6. **空 `changes` 不消耗 seq**。既然不落条目，铸出来的 seq 就没有主人；留一个空洞会让
   「日志里 seq 不连续」既可能是空步也可能是 018 的截断，两种原因混在一起没法诊断。
7. **`Change`/`Entry` 额外 derive 了 `Debug` / `Clone` / `PartialEq`**（`History` derive
   `Debug` / `Clone`）。纯加法，不改钉死签名，`assert_eq!` 要用。
8. **`History` 没有 derive serde**。修正明确只要求 `Change`/`Entry` 可序列化，而
   `History` 的 `next_seq` 是它自己维护的不变量——给它一个 `Deserialize` 等于允许外部
   构造出 `next_seq` 与 `entries` 不一致的日志。010 真要整份存取时，要么存
   `Vec<Entry>`（`entries()` 已经够用），要么那时候连同「恢复后 seq 从哪继续」一起设计。
9. **没有给 `record_set` 加 `#[must_use]`**，尽管丢弃返回值 = undo log 上一个静默空洞，
   正是它该管的事。原因是本 issue 有并行的独立测试 agent 按字面签名写验收，`-D warnings`
   下一行 `record_set(..);` 就会炸他们的构建，而这是他们没被告知的加法。**建议 017 的
   command 层加上**（那时调用方全在仓内）。

### 「derived 不产生 Entry」是怎么被证明的

不是「测试里没看见」，是**结构上不可能**：derived 的值由 store 内部
`flush_pending` → `dependencies_change` → `read_atom` 重算，那条路径整条不经过
`history/`。`record_set` 唯一能记的就是调用方显式点名的那个 atom。
`derived_recompute_produces_no_change` 把这件事钉成可回归的断言：下游确实重算了
（`debug_recompute_count` 涨了、`d` 从 2 变 10），而 `History` 里只有 `p` 那一条 change。
配套的 `writable_derived_still_records_only_what_it_was_asked_to` 说的是同一件事的反面——
writable derived 的 write fn 往 backing primitive 写的那一笔**没人记录**，想让它可回滚
就得由 command 层逐个 `record_set`，指望 store 帮忙捕获是不成立的。

### 自测（15 个单测 + 1 个 doctest，全部内联在模块里，没建 `tests/` 新文件）

`log.rs`：空日志形态、seq 从 0 严格递增、空 changes 不落条目也不烧 seq、
entries 顺序与 meta 透传、一条 entry 多处变更、`Entry`/`Change` serde 往返
（**键用 `String`，本 crate 的测试里不出现 `AtomId` 作为 K**）。
`record.rs`：prev/next 正确、prev 是活值不是 init、值相等返回 `None` 且日志为空、
相等判定走 `PartialEq`、一次 batch 两处写 = 一条 entry、batch 内同一 atom 连写两次
prev→next 首尾相接、derived 重算不产生 change、writable derived 的连带写入不入日志。

### 验收结果

- `cargo test -p agent-store`：15 unit + 45 集成 + 1 doctest 全绿（原有 45 个一字未改）
- `cargo test --workspace`：483 passed / 0 failed（009 之前是 467，+16）
- `cargo clippy --workspace --all-targets -- -D warnings`：0 警告
- `bash scripts/check-invariants.sh --all`：红线检查通过（含红线 4 对新文件的检查）

### 合并记录（主会话）

双侧零分歧合流：钉死签名一字未改，独测 9 测试 + 实现 15 自测 + 1 doctest 全绿，
workspace 492/0。两个亮点进档案：①log/record 拆文件是红线 4 检查器逼出来的
正确设计（AtomId 与 Serialize 不同文件成为结构事实）；②「K 不用 AtomId」在独测里
是编译期证明（类型不出现）不是运行时断言。裁决：record_set 的 #[must_use]
推迟到 017 command 层一起加（并行独测正在按字面签名构建，现在加是背刺）；
首条 seq=0、泛型序 <V,K> 维持。History 不 derive serde（next_seq 是自身不变量，
整份存取归 010 连同恢复后发号一起设计）——收。
