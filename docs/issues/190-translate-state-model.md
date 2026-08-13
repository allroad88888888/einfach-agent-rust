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

### 待办

- [ ] 中文版更新时三份都会滞后。三份都译完了，现在可以判断要不要加机制——
      **倾向不加**：三份文档的更新频率都很低（都是接缝文档，不是每天动的东西），
      而一个「标题数对不上就提示」的检查会在任何一次正常的中文侧小改上误报。
      改成在 [WORKFLOW.md](../WORKFLOW.md) 的收工检查里加一句提醒更合适——
      **但那属于「没有验证的约定」**（[185](185-post-invariants.md) 那篇文章的靶子），
      所以先如实记着这个矛盾，等真出现一次滞后再定
