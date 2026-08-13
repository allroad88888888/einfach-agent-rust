# 190 英译 `STATE-MODEL.md`

**里程碑** L · **依赖** [188](188-translate-invariants.md)（沿用术语表） · **模型** sonnet · **估时** 20min · **状态** 完成（2026-08-13）

## 目标

463 行，三份里最长的一份，也是**这个项目全部差异化的技术底座**——
一句话定位里那句「undo / redo / crash recovery / audit replay 是同一套机制的四个投影」
的证明就在这份文档里。

有人会因为不信那句话而点进来查证。**查证路径断在中文上，那句话就白说了。**

## 做什么

`docs/STATE-MODEL.en.md`，规矩同 [188](188-translate-invariants.md)。

重点译准的段落：

1. **原子图 + command log** 的基本机制
2. **「四个投影」那段的论证**——为什么持久化与 undo 是同一份代码
   （决策 6：恢复 = 从快照把 `next` 往前推，那就是 redo 的循环）。
   **这段是全篇的支点**，译砸了整份文档失去意义
3. **§子 agent**：整棵树共用一个 store、family 按 `AgentId` 区分、
   子读父是一次 `get`、跨 agent undo 天生一致
4. **可逆性屏障**：不可逆操作前面挡着，显式 override 仍在

## 验收

- 「四个投影」那段的论证链在英文里同样成立（自己读一遍能不能被说服）
- 术语与 [188](188-translate-invariants.md) 对照表一致
- 篇幅长，**分批译分批检查**，别一口气译完再看
- 内部链接不留死链

## 之后

三份译完，[179](179-readme-rewrite.md) 里三个钩子的证据链接可以全部指向英文文档，
英文读者的查证路径首次完整闭合。

---

## 实做记录（2026-08-13）

`docs/STATE-MODEL.en.md`。三份里最长的一份，规矩同 [188](188-translate-invariants.md)/[189](189-translate-architecture.md)。

### 「四个投影」那段的论证是全篇支点，译时当成论证而不是描述

本 issue 验收第一条：「自己读一遍能不能被说服」。落地时把它拆成一条可跟的链：

> 完整状态 = 所有 primitive atom 的值 → 快照就是序列化它们 → 恢复就是灌回去 +
> derived 自动重算 → **回滚后所有派生值自动一致**

最后那句译成 "The hand-written-state-machine bug where a rollback misses a field
**cannot structurally occur here**"——中文原文是「结构上不存在」，
`cannot structurally occur` 保住了那个「不是我们很小心，是它没法发生」的意思。

决策 6（恢复 = redo）译成 "**That is the redo loop, the same function** — there is no
second loading path"，紧跟着接红线 1 的根据（重放要能得出同样的结果）。

### 十处「诚实标注」逐处核对

这份文档最值钱的地方是它**大量标注了没落地的东西**。译丢了它就从「诚实的设计文档」
变成「吹牛的架构图」。逐处核对：

| 处 | 英文落点 |
|---|---|
| `SkillsActive` 留壳 | "Kept as a shell, read-only" |
| 三个槽位无写入点 | "still have no write path" |
| `AtomKey::ToolCall` 无写入点 | "no production code writes it" |
| 六个 derived 未落地 | "have not landed (written here so nobody assumes they already work)" |
| `skills_active` 输入已过时 | "is obsolete" |
| `ToolCallSlot` 那支无写入点 | "no production write path" |
| `ToolCallSlot::Request` 至今没落地且是刻意的 | "it has never landed, and that is deliberate" |
| 中断语义上半表 ⛔ | "⛔ input missing" ×3 |
| 上半表没有落盘依据 | "currently has no on-disk basis" |
| source 槽位不 lazy 建 | "not created lazily" |

**核对方式我一开始用错了**：单行 `grep` 查关键词，两处返回 0，我却直接说「全部有落点」。
实际是那两处的措辞被换行断开（`no production\nwrite path`），grep 抓不到。
结论是对的，但**当时的验证方式支撑不了那个结论**——补查了原文才算数。

### 结构核对

`##` 11/11、`###` 12/12。三份英文文档的链接用脚本逐条 resolve，全通。

### 几处译得费思量的

- **「`ToolsAllowed` 一个槽位身兼两职不是省事」** → "isn't a shortcut"，
  并保住后面那句「三种情况**就是**同一种状态」（they **are** the same state）
- **⚠️ `begin_turn()` 那段**（073 踩的坑）：「症状是**静默的**……离现场十万八千里」
  → "the symptom is **silent** … nowhere near the scene of the crime"。
  这段是整份文档里最实用的一条经验，语气不能译平
- **「开放一个方向要有理由，封闭不需要」** → "opening a direction requires a reason;
  closing one doesn't"
- **`load` 三态那段**：「第一张快照就把用户原文件覆盖了」译成 "the first snapshot
  overwrites the user's original file"，并保住「损坏之前还能人工修复的数据这下真没了」
- **「链通、值错、不报错」**（evict 第 3 条）→ "chain intact, value wrong, no error"

### 三份英译全部完成

[188](188-translate-invariants.md) / [189](189-translate-architecture.md) / 本条。
英文读者的查证路径首次完整闭合：README 的三条文档链接现在都指向英文版。

### 待办 → 已定（2026-08-13）

- [x] ~~中文版更新时三份都会滞后，要不要加机制~~ ——
      **定了：既不加门禁，也不加口头约定，改成「钉住译自哪个 commit」。**

三份英译顶部的那段提示各加两行：

> Translated from [STATE-MODEL.md](STATE-MODEL.md) **as of commit `064126a`**.
> …
> To find out whether it has lagged, and by how much:
> `git log --oneline 064126a..HEAD -- docs/STATE-MODEL.md`. Empty output means this
> translation is current. If you update the translation, move the hash.

三份的 hash 分别是 `9ae84d5` / `5e45a2a` / `064126a`，逐条验过命令**当前都输出空**。

#### 为什么这个形状，而不是前面记的那两个候选

原来记的两个候选各有一处过不去：

| 候选 | 过不去的地方 |
|---|---|
| 「`##` 标题数对不上就提示」的检查 | 中文侧任何一次正常小改都误报。**一个天天误报的门禁，
三周内一定会被加到忽略列表里**，那时它比没有更糟——它还占着「我们有检查」的位置 |
| 在 WORKFLOW.md 收工检查里加一句提醒 | 「没有验证的约定」，正是 [185](185-post-invariants.md)
那篇文章自己立的靶子。在自己仓里干一遍就更难看 |

钉 commit 这个形状两边都躲开了，因为**它不是规则，是数据**：不拦任何人、不需要谁遵守，
只是把「滞后了吗」从一个要靠人记得去做的比对，变成一条随手能跑的命令。

#### 真正决定选它的是失败方向

这才是关键，跟本仓十二条红线是同一条判据——**看它错的时候朝哪边错**：

- 有人改了中文、也重译了、但**忘了挪 hash** → 命令报出几条改动，去看，发现已经译过了。
  **误报，代价是读一遍 diff。**
- 有人改了中文、**没重译** → 命令如实报出来。**正确。**
- 没人动 → 空。**正确。**

三种情况里**没有一种会静默地说「一切同步」而实际不同步**。而那个标题数检查恰恰会：
中文侧改了一整段内容但没动标题结构，它照样绿。

**「有可能误报」和「有可能静默漏报」不是同一个量级的缺点。** 这是红线 1–6、11、12
那六条共用的判据（错了不报错、在 undo 或恢复时才以错值浮出来），
这次只是把它用在了文档上。

#### 一处诚实的限制

hash 是人手写的，**这个机制本身也可能滞后**。但它滞后的后果只是「hash 停在更早的
commit」，于是命令报出更多改动——**滞后让它更吵，不是更安静**。
一个会随着被忽略而变得更显眼的标记，比一个被忽略后就消失的提醒可靠。

（[188](188-translate-invariants.md) 里同一条待办指向这里，不重复记。）
