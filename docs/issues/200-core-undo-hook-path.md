# 200 core：undo 路先跑还原钩子，再回滚状态

**里程碑** M19 · **依赖** [199](199-reversibility-as-delivery-decision.md)（拍板） · **模型** **opus** · **独测** ✅ · **状态** 完成（见文末，2026-08-17）

## 目标

把决策 199 的第三、五、九条落进 `agent-core` 的 undo 路：**还原钩子在 `apply_prev`
之前逐条逆序跑，失败转 `Blocked`，`barrier: bool` 扩成三态 `Undoability`。**

**core 仍然不认识 `UndoFn`**——它收一个调用方递进来的回调，自己不持有闭包、不做 IO
（红线 7）。

## 做什么

### 1. `EntryMeta.barrier: bool` → `Undoability` 三态

```rust
pub enum Undoability {
    StateOnly,   // 没碰外部世界——状态回滚就够了
    Hooked,      // 碰了，且交了还原函数
    Blocked,     // 碰了，没交还原函数 —— 屏障
}
```

- `meta::is_barrier` 改成 `matches!(meta.undoability, Undoability::Blocked)`；
- `Txn::mark_barrier`（`txn.rs:111`）改成置 `Blocked`，另加一条置 `Hooked` 的路；
- **落盘 schema**（`persist/meta.rs`）：写出新字段；读入时对**老会话文件**做逐字确定
  的迁移——`barrier: true → Blocked`、`barrier: false → StateOnly`。老会话本来就没有
  钩子，这个映射对它们是真的（199 §九）。**迁移必须有独测，用真的老格式字节。**

### 2. undo 路收一个还原钩子执行器

```rust
pub enum HookOutcome {
    /// 没有钩子要跑，或钩子跑成功了 —— 可以回滚这一条的状态
    Ok,
    /// 钩子跑失败了 / 钩子已随进程重启消失 —— 停在这一条
    Failed(Arc<str>),
}

pub fn undo_turn_with(&mut self, run_hook: &mut dyn FnMut(&AgentEntry) -> HookOutcome) -> UndoReport;
```

`undo_step` / `undo_turn_force` 同款。**旧的无参版本保留为「递一个恒 `Ok` 的钩子」的
薄封装**——CLI 之外还有 wasm / server / 测试在调，一次全改是无谓的爆炸半径。

回调收 `&AgentEntry`（不是 `&EntryMeta`）：runtime 要靠 `Entry::seq` 查表，而 `seq`
在 `Entry` 上不在 `meta` 上（199 §九：**能不加字段就不加**，所以没往 `EntryMeta` 塞
`call_id`）。

### 3. 顺序：钩子先跑，成功了才 `apply_prev`

**这是本 issue 唯一会静默出错的地方**（199 §注意）。写成：

```
逆序遍历这一批 entry：
    outcome = run_hook(entry)
    若 Failed  → 停，产出 Blocked{ 已退了几条, 卡在 seq, 成因 }，
                 **这一条的状态不回滚**（199 §五）
    若 Ok      → apply_prev(这一条)
```

不许写成「先 `apply_prev` 一整批，再回头跑钩子」——那样还原失败时 store 已经说
「没发生过」而外部世界还在，正是红线导言点名的静默错值。

### 4. `UndoReport::Blocked` 加成因

```rust
Blocked { entries: usize, barrier_seq: u64, cause: BlockedCause }

pub enum BlockedCause {
    /// 这一步没交还原函数 —— 今天的屏障，**没碰**
    NoHook,
    /// 钩子跑了但失败了 —— **碰了，可能做了一半**
    HookFailed(Arc<str>),
    /// 钩子已随进程重启消失（`Hooked` 但表里查不到）
    HookLost,
}
```

三种给用户的话术不同，这正是加成因的全部理由（199 §五）：屏障是「没碰」，
后两种是「碰了」，用户据此决定要不要 `undo_turn_force`。

### 5. `redo` 不动

`redo_turn` 今天没有屏障参数，理由写在 `undo.rs`：「redo 只是把值写回状态，不重放
外部副作用」。**这条不变**——redo 不跑还原钩子，也不跑正向钩子。

## 验收

- **顺序钉死**（本 issue 最硬的一条）：一个 fake 钩子在失败时**记录当时 store 里那条
  atom 的值**；断言它读到的是**回滚前**的值。写反了这条必红。
- 钩子失败 → `Blocked{ cause: HookFailed }`，比它新的那些**已经退掉**，失败那条
  **停在新值上**（读一次 atom 断言）。
- `undo_turn_force()` 越过失败那条继续退；**只越过一条**（同一批里有第二条失败的
  再停一次）——沿用 `undo_turn_force` 已有的「一次确认放行一条」语义，加一条测试
  钉住它对 `HookFailed` 也成立。
- `Hooked` 但钩子表空（模拟恢复后）→ `Blocked{ cause: HookLost }`，不是静默跳过。
- **老会话文件迁移**：一份 199 之前格式的真实会话文件，恢复后 `barrier: true` 的那条
  仍然挡 undo，`barrier: false` 的那些仍然不挡。
- 逆序：三条带钩子的 entry，断言钩子被调用的顺序是 `seq` 降序。
- 无参 `undo_turn()` 薄封装的行为与 199 之前**逐字节相同**（既有 undo 测试全绿，
  一条断言都不改）。
- `cargo test --workspace` 全绿 + `check-invariants` 过 + `build-wasm` 绿。

## 注意

- **红线 6 在这条路上**：`rewind` 的三步（挪游标 → bump 世代 → 写回状态）顺序不能换，
  钩子插在哪一步之后要想清楚——**钩子跑之前必须已经 bump 过世代**，否则钩子跑的时候
  一条在飞回执可能刚好落地写进一个正在被撤销的世界。
- `undo.rs` 今天 216 行，加这些会顶到红线 9。**拆分是本次改动的一部分**：
  `undo.rs` 留 `undo_*`/`redo_*` 的公开口，钩子执行与 `Blocked` 成因判定拆去
  `undo_hook.rs`。照 `spawn.rs`/`despawn.rs`/`restore.rs` 的既有先例。
- 别顺手给 `EntryMeta` 加 `call_id`。199 §九 算过账：`seq` 够用，加字段要动落盘 schema。
- 别把「逆序」优化成并行。199 §四：逆序是论文 Theorem 16 里**唯一不需要前提**的顺序，
  任意顺序要求 effects 两两独立，而那是我们无法验证的性质。

## 实做记录（2026-08-17，review 已过）

三门禁全绿：`cargo test --workspace` 2142 passed / 0 failed；`check-invariants --all`
退出码 0，13 条红线 9 提示**与基线逐条相同**（本次新增/改动的文件一个都没被点名）；
`build-wasm.sh` 绿。

**顺序那条是这么钉死的**（本 issue 最重要的一条）：夹具造三条 `TurnsUsed 0→1→2→3`
的 entry，`Hooked` 在 seq=1，钩子失败时读一次 store。三种实现读数互不相同——
**正确 = 2**、「先 `apply_prev` 一整批再回头跑钩子」= 0、「这一条先退再跑它的钩子」= 1，
断言消息把两种写错都点名。review 时**独立注入过第三种写法**：9 条测试红 6 条，
关键那条报 `left: Some(1) / right: Some(2)`。

**老会话迁移**用的是**真的完整 journal 字节**（`tests/it/legacy_barrier_migration.rs`
里三行真产物，只把 meta 那段换回 `"barrier":true/false`），不是手搓结构体。
`PersistedMeta` 走 `#[serde(from = "RawMeta")]` 两版字段都收；**特意没加
`#[serde(default)]`**——那会把老文件的 `barrier:true` 读成 `StateOnly`，正是
「一次真实不可逆操作从此不再挡 undo」的静默错值。idb 那条 wasm 持久化路已确认
**没有第二份 meta 序列化**（`Record<K,V,M>` 全程泛型透传，`M` 就是同一个
`PersistedMeta`），迁移写一处两个后端一起生效。

### 实现期发现、issue 原文没预见的五条（review 逐条确认，均采纳）

1. **`HookOutcome` 要第三个变体 `Lost`**。core 不知道 runtime 表里有没有，两态表达不出
   「查不到」，压成 `Failed` 会让成因退化。
2. **钩子只对 `Undoability::Hooked` 发问**（issue 伪码是无条件 `run_hook`）。无条件的话
   runtime 对每条 `StateOnly` 都查不到表、返回 `Lost`，**一条干净的 entry 当场变成障碍**。
   副产品：无参 `undo_turn()` 与 199 之前逐字节等价成了结构性的，有测试钉。
3. **`/undo!` 的放行额度必须在逐条循环里消费，不能在 `History` 谓词里**。谓词只看得见
   `&EntryMeta`、只认得出 `Blocked`，会乐观放过一条**更老**的屏障；额度若就此算花掉，
   一条更新的失败钩子永远越不过去——用户按多少次 `/undo!` 都停在同一处（**活锁**）。
4. **`rebuild_touched_agents` 从「整批一次」改成「按条，就在这条 `apply_prev` 之前」**。
   整批补会给一批**不会被回滚**的 entry 也建图：其中若有一条 despawn，那个子 agent 会
   带一整套默认值复活进 family，`primitives()`/快照凭空多一个 agent 且全程不报错。
5. **`Session::mark_hooked`**（issue 只说「另加一条置 `Hooked` 的路」）。只加 `Txn` 那半是
   没有入口的死代码，也没法用真事件造出 `Hooked` entry 来测。它同时是 **201 会直接踩上来
   的接缝**：决策 34 的 `Aftermath` 三态一一对应（`Nothing` = 都不标 / `Undo(f)` =
   `mark_hooked` / `Irreversible` = `mark_irreversible`）。

### 顺手改了三段文档（issue 没列，但不改就是假话）

`STATE-MODEL.md` §「Command log」的 `EntryMeta` 代码块与「唯一落盘依据」那段、
`STATE-MODEL.en.md` 同款、`docs/TOOLS.md` §屏障那段——原文都写着「`barrier` 是 undo 屏障
的唯一落盘依据」。**代价记在 [203](203-reversibility-docs-cleanup.md) §8**：203 的盘点草稿
是在这之前做的，`TOOLS.md` 行号已后移。

### review 时回退的一批改动

实现期跑过一次 `cargo fmt`，把 **11 个与本 issue 无关的存量文件**一起重排了
（HEAD 本来就不是 fmt-clean）。已全部 `git checkout` 回退——超出 issue 范围
（`CLAUDE.md`：路过存量文件不擅自顺手重构），而且它把 `clear_tool_results.rs` 推到
326 行、`http/capabilities/assemble.rs` 推到 301 行，**凭空造出两条红线 9 违规**。
回退后两者回到 300 / 299。

`is_replayable()` 按 199 §八 要删，但**不在本 issue 范围**，留给
[202](202-host-mcp-undo-none.md)。
