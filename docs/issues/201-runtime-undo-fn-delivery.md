# 201 runtime：工具干完活交还原函数，钩子表按 `seq` 记账

**里程碑** M19 · **依赖** [200](200-core-undo-hook-path.md) · **模型** sonnet · **独测** ✅ · **状态** 未开始

## 目标

决策 199 第一、二、六条的落地面：**执行体的签名从「返回正文」改成「返回正文 + 可选的
还原函数」**，runtime 维护 `seq → UndoFn` 表并接上 200 的回调。

这是本里程碑真正改变能力的一条——199 之前"可逆"是个字符串，这条之后它是个能被调用的
东西。

## 做什么

### 1. 签名

```rust
pub type UndoFn = Box<dyn FnOnce() -> Result<(), Arc<str>> + Send + Sync>;

/// 一次调用在**外部世界**留下了什么（199 §一）。**三态，不是 `Option<UndoFn>`**——
/// `Option` 会把「没碰」和「碰了但撤不回」压成同一个 `None`，而落盘那一位是三态，
/// 返回类型必须与它同构。
pub enum Aftermath {
    Nothing,           // → Undoability::StateOnly
    Undo(UndoFn),      // → Undoability::Hooked
    Irreversible,      // → Undoability::Blocked
}

// 截获式工具（EXTENSIONS.md §四 的正门）
pub type SessionToolFn = Box<
    dyn Fn(&mut Session, &AgentId, &Value) -> Result<(Arc<str>, Aftermath), Arc<str>>
    + Send + Sync>;
```

`Aftermath`（runtime 词汇，工具交代的事实）→ `Undoability`（core 词汇，这条 entry 的
记账）的翻译在**宿主侧**做，1:1 映射见上面注释。两个类型**不合并**：core 不认识
`UndoFn`（红线 7）。

`FnOnce`：还原只跑一次。跑第二次等于「在一个没有对应应用的状态上跑一个逆」，
论文 §5.1.1 的 `armed` 标志防的就是这个——我们用类型防，更便宜。

**`ExtensionPack::with_tool` 的第二个位置参数（`Reversibility`）删掉。** 可逆性从此
由执行体的返回值决定，不由注册时的声明决定。EXTENSIONS.md §「可逆性：没有缺省」那一段
的judgement 保留（拿不准就别交函数），但落点从「填一个枚举」变成「交不交函数」。

### 2. 钩子表

`RunnerCtx` 上挂 `BTreeMap<u64, UndoFn>`（键是 `Entry::seq`），类比在飞 provider
凭据表与 `McpRegistry` 的既有形状——**活句柄住 store 外**（红线 3）。

登记时机：工具结果落地、`commit` 产生 entry **之后**，读一次 `Session::last_entry()`
拿 `seq` 登记。这跟 `mark_irreversible` 今天「派发时登记 call_id、结果落地时翻译成
entry 上的位」是同一条路的镜像，**不新发明记账**。

清理时机：entry 被 `History` 的 cap 挤掉时，对应的钩子也该丢——否则长会话里这张表
只涨不落。`DEFAULT_HISTORY_CAP` 是 100，所以上限有界，但**要显式清，不许靠"反正有界"**。

### 3. 接上 200 的回调

`agent-cli` / `agent-server` / `agent-wasm` 调 undo 的地方，从 `undo_turn()` 换成
`undo_turn_with(&mut |entry| …)`，回调里按 `entry.seq` 查表：

| 表里 | entry 的 `Undoability` | 回调返回 |
|---|---|---|
| 有 | `Hooked` | 跑它；`Ok(())` → `Ok`，`Err(e)` → `Failed(e)` |
| 没有 | `Hooked` | `Failed("还原函数已随进程重启消失")` → 200 的 `HookLost` |
| 没有 | `StateOnly` / `Blocked` | `Ok`（`Blocked` 那条根本走不到回调，屏障谓词先拦） |

### 4. 既有执行体逐个跟签名

| 工具 | 交什么 | 落成 | 理由 |
|---|---|---|---|
| `srv:agent/spawn` | `Nothing` | `StateOnly` | 199 §六（**已按 201 落地时的发现修正**）：它在**外部世界**留下的是零，子 agent 状态活在同一条日志上、回滚父那步就跟着回滚。判 `Hooked` 会让恢复后查不到钩子 → `HookLost` → 每次 spawn 都变屏障 |
| `srv:fs/read`、`fs/list`、`status`、`collect`、`skill/read` | `Nothing` | `StateOnly` | 纯读，没碰外部世界。**这跟 `Irreversible` 的区别是本次改动的全部要点**——不是「碰了但撤不回」 |
| `srv:shell/exec` | `Irreversible` | `Blocked` | 行为与今天逐字相同 |
| `ext:stats` 的截获工具（`agent-cli/src/ext_stats.rs`） | `Nothing` | `StateOnly` | 纯读；教材要同步 |

### 5. `mark_irreversible` 的去留

保留，但语义收窄成「这次调用交不出还原函数」的登记口。**名字要改**（`mark_no_undo`
或类似）——`mark_irreversible` 这个名字来自枚举时代，留着会让人以为还有个
`Reversibility` 在背后。改名是机械的，一并做。

## 验收

- **端到端真跑一次**（本 issue 的主验收，不是 mock）：写一个 fake 扩展工具，
  执行时在 `scratchpad` 建一个文件、交回「删掉它」的还原函数。
  1. 调它 → 文件在
  2. `/undo` → **文件没了**，`UndoReport::Applied`
  3. 让还原函数真的失败（**别用 chmod 只读**：Unix 删文件看父目录权限，root 还绕过权限位，
     容器里以 root 跑 CI 会静默变成「删成功了」；201 落地时改用 `remove_dir` 撞非空目录），
     重跑一次 → `Blocked{ HookFailed }`，**文件还在**，
     且那条 entry 的状态**没回滚**
  4. `/undo!`（force）→ 越过它继续退，文件仍然在（用户已确认接受）
- 交 `Irreversible` 的工具行为与 199 之前逐字节相同（`shell/exec` 的既有屏障测试一条不改）。
- spawn 仍然不挡 undo（`subagent_parallel.rs:213` 那条断言不改，但理由从
  「它是 `Reversible`」变成「它交了 `Aftermath::Nothing`，在外部世界什么都没留下」——注释要跟上）。
- 钩子表随 history cap 清理：跑满 `DEFAULT_HISTORY_CAP + 10` 条带钩子的 entry，
  断言表长 ≤ cap。
- `cargo test --workspace` 全绿 + `check-invariants` 过 + `build-wasm` 绿
  （`SessionToolFn` 是公开类型，wasm 侧编译要验）。

## 注意

- **`UndoFn` 是 `FnOnce` 且 `Send + Sync`**：扩展作者会想在里面捕获执行时的现场
  （旧文件内容、创建出来的资源 id）。这正是设计意图——199 §一「逆是在执行的那个状态
  上选的」。但它**不能捕获 `&Session`**（生命周期不允许，也不该）：还原函数只管外部
  世界，状态那半边由 journal 回滚承担。这条要写进 EXTENSIONS.md。
- **别让还原函数拿 `&mut Session`。** 它跑在 undo 路上，那时 core 正在回滚状态；
  让它同时写状态就是在一次回滚中间插一次前向写入，红线 6 的账当场乱掉。
- 别顺手给 `TimedRun` 也加还原函数。timed 钩子的副作用**本来就不进 command log**
  （153 决策 30 / `turn_end.rs` §审计面），没有对应的 entry 可挂，是另一件事。
- 宿主工具 / MCP 这条路上**什么都不做**，那是 [202](202-host-mcp-undo-none.md) 的范围。
  （202 的判据 review 时也修正过：**事实可以采信，承诺不能空转**——`pure`/`readOnlyHint:true`
  声明的是「没碰外部世界」这个事实，落 `StateOnly` 不挡；`reversible` 声明的是一个交不出
  函数的承诺，落 `Blocked`。见 199 §七。）
