# 008 按职责拆分 store.rs

**里程碑** M2 · **依赖** 015 · **模型** sonnet · **独立测试 agent** 否 · **状态** 完成

## 目标

上游 `store.rs` 是 1297 行，顶穿红线 9 的 500 行硬上限。按职责拆成五个文件。

## 参考切法

- 图结构与 atom 记录
- read 求值路径
- flush + pending 调度
- 订阅分发
- debug 内省

按**职责**拆，不按行数凑。检验标准：能用一句不含「和 / 以及」的话说清每个文件是干嘛的。

禁止的拆法：`store2.rs`、`part1.rs`、往 `utils.rs` 里塞；也不为凑行数把强内聚的
逻辑打碎。

## 验收

- 每个文件 ≤300 行（`wc -l` 口径）
- hook 与 `--all` 都通过
- 007 移植过来的测试仍然全绿——**拆分不改行为**

## 注意

Rust 惯例把 `#[cfg(test)] mod tests` 内联在底部，会显著抬高行数。本仓取向是把集成测试
挪到 `tests/`，源文件只留最贴身的单元测试。**不要靠删测试来压行数。**

## 实做记录（2026-08-01）

`store.rs`（1310 行）拆成 `store/` 目录下 10 个文件（1 个 `mod.rs` + 9 个子模块），
`lib.rs` 的 `pub use store::{AtomValue, Store, CellListener, ReadArgs, WriteArgs,
SubscriptionId}` 一行没动——子模块全部声明成私有 `mod`，只在 `mod.rs` 里 `pub use`
需要对外的那几个名字，`store::records::Foo` 这类路径外部不可达，公开面比拆分前更收敛
而不是更宽。跨子模块访问走 `pub(super)`（可见范围 = `store` 及其全部子模块，等价于
拆分前"模块私有"字段在文件级别的展开），没有新增任何面向 crate 外部的可见性。

### 最终文件清单

| 文件 | 行数 | 职责（一句话） |
|------|-----:|------|
| `store/mod.rs` | 83 | 模块级文档（store.ts 对照表 + DV-1~4 偏差清单）和子模块的公开面收口 |
| `store/records.rs` | 196 | atom 记录与依赖图的数据结构（`AtomRecord`/`BackDeps`/`Inner`）及其不可再分的存取原语 |
| `store/handle.rs` | 106 | `Store` 句柄的构造，以及往记录表里分配注册一个新 atom |
| `store/eval.rs` | 159 | DV-3 迭代式求值状态机本体：`read_atom` 的显式帧栈、`commit_read`、`seed_primitive` |
| `store/read.rs` | 208 | 喂给 derived 读函数的追踪/免追踪访问口：`ReadArgs`、`Scratch` 暂存区、`read_dep` |
| `store/flush.rs` | 283 | 写入落地后的 pending 调度与依赖传播：`WriteArgs`、`PendingQueue`、`set_atom_state`、`dependencies_change`、`flush_pending` |
| `store/subscribe.rs` | 104 | atom 变更的订阅登记与分发 |
| `store/graph.rs` | 149 | 面向 engine adapter 的依赖图结构查询与 atom 收尾（`reverse_*`/`direct_*`/`invalidate`/`destroy_atom`/`clear`） |
| `store/guards.rs` | 115 | 四个 RAII 守卫：panic 不清空 `computing`/`read_depth`/`setting`/`batch_depth` 就会永久卡死 |
| `store/debug.rs` | 69 | 面向诊断的只读探针（`#[doc(hidden)]`） |

全部 ≤300 行（本 issue 的验收线；`flush.rs` 283 行最接近上限）。没有用 INVARIANTS.md
红线 9 "复杂文件 ≤500" 的豁免——凡是顶到 300 的地方都进一步拆了，而不是找理由不拆。

### 归属最难定的几段代码

1. **`read.rs` / `eval.rs` 的切分点**——`ReadArgs`/`Scratch`/`read_dep`/`read_atom`/
   `commit_read` 原本是同一份"帧栈 + 故障重跑"协议，天然是一个算法，2026-08-01 复核时
   INVARIANTS.md 红线 9 明确允许这类"强内聚单一状态机"到 500 行，但本 issue 的验收标准
   写死 300，没有对应豁免，所以还是拆了。切法：`read_dep`（一次 `get`/`peek` 该怎么答）
   留在 `read.rs`；`read_atom`/`commit_read`（整棵帧栈怎么算完）挪到 `eval.rs`。代价是
   `ReadArgs`/`Scratch` 的部分字段从"文件私有"升级成 `pub(super)`，`read_dep` 递归到
   `eval::read_atom` 变成一次跨文件调用——都是机械改动，两个文件各自的读者仍然只需要
   理解"一份协议"，没有被打散成互相看不懂对方在干嘛的碎片。
2. **`commit_read` 该归 `eval.rs` 还是 `records.rs`**——它同时做两件事：按 diff 更新
   依赖边（图结构，理应归 `records.rs`）、调用 `set_atom_state` 落值并计数（归
   `flush.rs`）。最终跟着 `read_atom` 留在 `eval.rs`，理由是它是"一帧读完成时的唯一
   出口"，和 `read_atom` 共享同一次帧栈迭代的上下文（faulted 就丢弃、否则提交），
   拆到别处会让 `read_atom` 里那段"commit or retry"的分支逻辑必须跳文件才能看全。
3. **`publish_atom`/`listeners_snapshot` 该归 `flush.rs` 还是 `subscribe.rs`**——从
   调用位置看它们是 `flush_pending` 的最后一步，像是"flush"的一部分；但它们的全部工作
   就是读 `subscriptions` 表、通知监听器，和 `sub`/`unsub` 操作的是同一张表。判给
   `subscribe.rs`（登记和分发是同一个"订阅"概念的两面），`flush.rs` 只在 drain 循环里
   `use super::subscribe::publish_atom` 调用它。
4. **`set_atom_state` 该归 `records.rs` 还是 `flush.rs`**——它是 `Inner` 上最基础的
   "写值"原语，直觉上该和 `record`/`is_fresh` 那批存取方法待在一起；但它存在的唯一
   理由就是往 `pending` 队列塞一条记录、累加 DV-4 的 `write_seq`——是"一次写入怎么进入
   调度"这条线的起点。最终放进 `flush.rs`，用第二个 `impl<V: AtomValue> Inner<V>` 块
   单独承载（Rust 允许同一个类型的 `impl` 块分布在不同文件，只要都在同一 crate 内）。

### 007 清理

`Inner::set_atom_state`（现 `flush.rs`）和 `Store::unsub`（现 `subscribe.rs`）里 007
留的两处 `#[allow(clippy::collapsible_if)]` 已按注释指示清理：前者把嵌套 `if let` 折成
`prev.as_ref() == Some(&value)` 一次比较；后者把嵌套 `if let` 改成两次 `let-else` 提前
返回。两处都是纯粹的控制流等价改写，未改变任何分支的可观察行为。

### 验收结果

- `cargo test -p agent-store`：45/45（跟 015 移植时完全一致，一个测试文件都没改）
- `cargo test --workspace`：467/0
- `cargo clippy --workspace --all-targets -- -D warnings`：0 警告
- `bash scripts/check-invariants.sh --all`：`scripts/check-invariants.sh` 里
  `crates/agent-store/src/store.rs` 的过渡豁免行（连同上面三行注释）已删除，脚本仍然
  全绿——红线 9 对这批新文件恢复了正常检查，不再是永久失效状态。

### 合并记录（主会话）

10 文件全部 ≤300、公开 API 字节不变、45 测试零改动全绿、**豁免行已删且红线照过**
——「拆分没改行为」的三重硬证据齐了。归属判断都有记录，007 的两个 allow 顺手清掉。
workspace 467/0。
