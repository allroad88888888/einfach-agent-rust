# 188 英译 `INVARIANTS.md`

**里程碑** L · **依赖** [165](165-launch-positioning-decision.md) · **模型** sonnet · **估时** 20min · **状态** 完成（2026-08-13）

## 目标

228 行。**三份英译里优先级最高的一份**——因为 [185](185-post-invariants.md) 那篇文章会指向它，
而且「红线」是这个项目最容易让工程师产生好感的东西：它证明作者知道什么会坏。

## 做什么

`docs/INVARIANTS.en.md`，与中文版**并存**（不替换）。中文版仍是权威，
英文版顶部注明「translated from INVARIANTS.md; the Chinese version is authoritative」。

**为什么并存而不替换**：这个仓的开发在中文进行，替换会让日常开发变慢，
而变慢的文档很快就会过期——[167](167-readme-stale-mechanism.md) 刚吃过一次过期文档的亏。

## 注意

1. **不要直译**。中文技术写作里的省略在英文里会读不通，尤其那些「靠上下文补主语」的句子。
2. 红线的**理由**比条目本身重要，理由段要译得特别准——译丢了它就退化成一张规则清单，
   而规则清单没人看。
3. 术语对照要跟 README 已用的英文一致：atom / derived / command layer / journal /
   reversibility barrier / prefix chunk。**建一个小对照表放文件底部**，
   后面 [189](189-translate-architecture.md)/[190](190-translate-state-model.md) 直接沿用。

## 验收

- 12 条齐全，每条的理由段都在
- 术语与 README 一致，且对照表建好了
- 「1–6 条错了不会立刻报错」那段总结要在——**它是全文最有价值的一句**
- 英文母语者读起来不像机翻（长句拆开、被动语态收敛）

---

## 实做记录（2026-08-13）

`docs/INVARIANTS.en.md`。中英并存，英文版顶部注明**中文为权威**并说清理由
（「开发在中文进行，这份会滞后；两者不一致时中文是对的、这份是 bug」）。

### 完整性是机械核对的，不是目测

| | 中文 | 英文 |
|---|---|---|
| 条目数 | 12 | 12 |
| `**规则**` / Rule | 12 | 12 |
| `**为什么**` / Why | 12 | 12 |
| `**违反后**` / Breaking it | 12 | 12 |
| `**检查**` / Check | 12 | 12 |

四段结构一段不缺。内部链接四条全部指向真实文件（`../probes/PROVIDERS.md`、
`ADAPTER.md`、`INVARIANTS.md`、`issues/024-cache-guard.md`）。

### 理由段是重点，这条 issue 说对了

本 issue 注意事项第 2 条：「红线的**理由**比条目本身重要，理由段要译得特别准——
译丢了它就退化成一张规则清单，而规则清单没人看。」

落地时最费劲的正是这些：

- 红线 2「为什么显式声明是唯一可行解」那段（自动捕获需要 O(被追踪 atom 数) 的常驻
  订阅，而本仓每个 agent 的每个槽位都是 family atom）——**这段不译，读者就会问
  「为什么不自动记录」**
- 红线 4 的孪生条款（捕获 `AtomId` 的 derived 在依赖被逐出重建后当场 panic，
  「幸而不是静默错值」）
- 红线 9 那段上游 `store.rs` 1297 行按职责拆成五个文件的具体拆法
- 红线 12 的「事前问能力 → 事后报调整」整段论证

### 建了术语表，给 [189](189-translate-architecture.md)/[190](190-translate-state-model.md) 沿用

文件底部 14 条对照。几个定得比较费思量的：

- **料单 → ingredients**（不是 "materials"）：它强调「未加工、未合并」，
  ingredients 自带这个含义
- **接缝 → seam**：本仓的专有概念，直译且在表里给了定义
  （「差异被有意吸收的边界」）
- **红线 → invariant / red line**：泛指用 invariant，指编号规则时用 red line
  ——文章和文档里两种用法都真实存在，不强行统一
- **时机 / `CallTiming` → call timing**，并在表里点明三个取值

### 挂进了两处入口

- `README.md` 的文档列表指向**英文版**（英文读者的查证路径），
  并加了一句 "the twelve rules whose violations produce no error"——
  文档列表里只有名字的话没人会点
- `CLAUDE.md` 文档地图那一行标注「英译并存，**中文是权威**」，
  免得后来人以为改英文版就够了

### 待办

- [ ] [189](189-translate-architecture.md) / [190](190-translate-state-model.md) 沿用本表的术语
- [ ] 中文版更新时**这份会滞后**。已在顶部写明，但没有机制保证——
      真要保证得加一条检查（比如两份文件的 `## ` 标题数对不上就提示）。
      **先不做**：现在只有一份译文，加机制是给一个还不存在的问题上工具；
      三份都译完再看要不要装
