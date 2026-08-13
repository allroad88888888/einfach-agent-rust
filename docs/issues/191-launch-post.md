# 191 首发帖：Show HN / r/rust 的文案与时机

**里程碑** L · **依赖** [179](179-readme-rewrite.md) + [183](183-post-providers.md) · **模型** **opus** · **估时** 20min · **状态** 文案就绪，**等你发**（2026-08-13）

## 目标

首发只有一次。前面所有 issue 都是为这一刻做的准备。

**用 opus**：文案、渠道、时机三件事都是判断活，而且**没有第二次机会**——
Show HN 同一个项目再发一次效果会差很多。

## 前置检查（一条不满足就别发）

- [ ] demo 链接可用且**当天验过**（[170](170-pages-workflow.md)）
- [ ] README 第一屏有 demo + GIF（[173](173-readme-demo-hero.md)）
- [ ] **有一条陌生人能真的走通的试用路径**——demo 链接算一条（[170](170-pages-workflow.md)，自带 key）。原案写的「零成本本地路径」已撤，见 [177](177-openai-compat-config.md)
- [ ] LICENSE 在（[166](166-license.md)）
- [ ] 至少一篇独立价值的文章已发并有反响（[183](183-post-providers.md)）
- [ ] **你有一整天能守在评论区**——这条最容易被低估。首发当天不回复评论，
      热度掉得比什么都快

## 做什么

1. **Show HN 标题**：说清是什么 + 一个反直觉的钩子。不要用形容词。
   方向：`Show HN: An agent runtime where undo actually removes the turn from the model's memory`
2. **正文**（HN 的规矩是短、第一人称、说清动机）：
   - 为什么做这个（做别的东西时撞上了什么问题）
   - 三个钩子，各一句
   - 直接给 demo 链接
   - **坦白限制**：单租户、`RedisRegistry` 未实现、MCP 的 OAuth 没做。
     HN 上藏短处会被扒出来，主动说反而加分
3. **r/rust 单独写一版**——那边关心的是 Rust 侧的东西（原子引擎、wasm 目标、
   零 unsafe？测试策略），不是 agent
4. **时机**：美西时间工作日早上。避开大新闻日

## 验收

- 两个渠道各有独立文案，**不是同一篇复制**
- 「坦白限制」那段在，且是真限制不是假谦虚
- 评论区常见问题预先准备好答案：跟 rig 有什么区别、为什么是 Rust、
  为什么只有中国模型、生产用了吗

## 需要用户

发帖本人。账号历史、发帖时机、要不要露真名都是你的决定。

---

## 实做记录（2026-08-13）

**两套文案在 `scratchpad/191-launch-posts.md`。** 帖子本身要你发——账号历史、时机、
要不要露真名都是你的决定，而且**发帖是外发动作**，我不代做。

### 两个渠道两套文案，不是同一篇复制

本 issue 明写了这条，落地时的分法：

- **HN 关心「这是什么、为什么反直觉」**——标题就是一个可验证的反直觉断言
  （*undo actually removes the turn from the model's memory*），读者第一反应该是
  「等等，别人的 undo 不是这样吗？」，那个疑问就是点进来的理由。正文约 280 词，
  接近 HN 的舒适上限，不再加。
- **r/rust 关心「Rust 侧你怎么做的」**——正文主体是五条 Rust 特有的取舍
  （store 故意不 Send/Sync、落盘键不用 `AtomId`、`imbl::Vector`、十二条红线里六条
  违反后不报错、core 里不许有能力位）。**一条 agent 话术都不讲**。

### 三处有意的写法

**① HN 正文里写了那个坑。** 「ask again *without using the word "undo"*（说了这个词
就等于把答案喂给模型——我测的时候就是这么拿到一次假阴性的）」。

写进首发帖是因为：**照着试却拿到假阴性的人，会认为这个项目在吹牛**。
与其事后解释，不如在他动手之前就告诉他。顺带它还传达了「这人自己踩过并记下来了」。

**② 「Honest about what it isn't」整段保留**，并引用了 ARCHITECTURE 里那句原文
——*"there is not one line of code behind this"*。
HN 上藏短处会被扒出来，主动说反而加分；而且那句是真的引用，不是姿态。

**③ r/rust 结尾主动请人挑最硬的地方**：
> especially if you think the not-Send/Sync store is a mistake; that's the decision I'd
> most like to be argued with about.

主动请求针对最贵决策的反驳，比防守姿态好，而且那确实是最想听到反馈的一条。

### 评论区预案写了六条

跟 rig 的区别 / 为什么 Rust / 为什么只有中国模型 / 生产用了吗 / 不 Send/Sync 是不是限制 /
这不就是 event sourcing 吗。

其中「生产用了吗」的答案是**「没有」**，并明写「别把 dogfood 说成生产验证」——
这条最容易在评论区被追问，而含糊一次就把前面所有诚实标注的价值抵消掉了。

### 前置检查：**六条里差两条，都不满足就别发**

| | 状态 |
|---|---|
| demo 链接可用 | ✅ HTTP 200（**发之前当天再验一次**） |
| README 第一屏有 demo + GIF | ✅ |
| 一条陌生人能走通的路径 | ✅ demo 自带 key，公网 CORS 已验（[170](170-pages-workflow.md)） |
| LICENSE | ✅ 双许可 |
| **至少一篇文章已发并有反响** | ❌ **五篇都是初稿** |
| **你有一整天守评论区** | ❓ 只有你知道 |

**建议顺序**：先发 [183](183-post-providers.md)（三家实测那篇——独立价值最高、
最可能被转），看反响，再 Show HN 并在正文里引用它。

Show HN 只有一次机会，带着一篇没人看过的文章去发，等于把最能证明「这人认真」的
材料浪费掉。
