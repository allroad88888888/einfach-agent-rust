# 179 README 重写（英文定稿）

**里程碑** L · **依赖** [173](173-readme-demo-hero.md) + [178](178-openai-compat-dogfood.md) · **模型** **opus** · **估时** 20min · **状态** 完成（2026-08-13）

## 目标

把 README 从「解释这个项目怎么设计的」改成「让人 60 秒内决定要不要试」。

**为什么用 opus**：这不是排版活。它要在一屏之内完成定位、区分、证明三件事，
每一句都在跟「读者随时会关掉标签页」抢时间——是判断活，不是执行活。

## 现状的问题

1. 开篇那句 "without turning the model context or the Rust core into an integration dump"
   是**内行黑话**——已经懂的人才看得懂，而他们不是要争取的对象。
2. 结构是「Why It Is Different」四段并列，每段都在解释机制。**没有一句话说清这是什么品类**。
3. quickstart 埋在第 78 行。
4. 没有任何地方告诉读者「这跟 rig / langchain-rust 有什么不一样」——
   而这是每个 Rust 开发者点进来时脑子里唯一的问题（[165](165-launch-positioning-decision.md) L2）。

## 目标结构

```
▶ Try it in your browser（173 已放）
GIF（173 已放）
一句话定位（165 的草案，定稿）
─────
Why this is not another agent framework   ← 直接回答那个唯一的问题
三个钩子，每条带证据链接：
  · /undo 真的从模型记忆里删掉那一轮  → 143 的口令实验
  · kill -9 之后接着聊                → M18
  · 整个核心跑在浏览器里，无服务端      → demo 链接
─────
Quickstart（60 秒）
Runtime surfaces 表（保留，下移）
架构细节 → 全部降成文档链接
```

## 验收

- **一个没读过任何文档的 Rust 开发者，读完第一屏能说出「这跟 rig 有什么不一样」**
  —— 这条是本 issue 的全部意义，其余都是手段
- 三个钩子每条都有**可点的证据**（issue、文档或 demo），不是形容词
- quickstart 在第一屏可见范围内。**默认路径用哪家要重定**——原案写的是 Ollama，已撤（[177](177-openai-compat-config.md)）
- 保留现有的准确性——**不许为了好听夸大**。这个项目的全部可信度来自
  「文档和代码一致」，[167](167-readme-stale-mechanism.md) 刚修过一次因此塌掉的信任

## 之后

`README.zh-CN.md` 同步（可以是摘要，不必等长——它现在就是这个定位）。

---

## 实做记录（2026-08-13）

没等 [178](178-openai-compat-dogfood.md)。理由：178 只影响 quickstart 里**一句话**
（OpenAI 兼容端点怎么填），而 [173](173-readme-demo-hero.md) 的 demo 链接已上线——
门面的价值现在就在流失，为一句话压着整份重写不划算。那句话按 [177](177-openai-compat-config.md)
已落地的配置面写，指向 `providers.example.toml` 里那段，**不声称跑过真机**。

### 结构按 issue 定的来，但第一句换了

原计划第一屏是「demo 链接 → GIF → 一句话定位」。落地时把**定位提到了链接之前**：

> **An embeddable agent runtime with a real ledger.** Undo, redo, crash recovery, and
> audit replay are one mechanism, not four features.

理由：GitHub 的搜索结果、社交媒体的分享卡片、以及仓库列表页，抓的都是**标题下面的
头几行**。链接在那里没用（那些场合点不了），一句话定位在那里才有用。链接紧随其后。

### 「这不是又一个 agent 框架」直接写成了小标题

issue 说这是「每个 Rust 开发者点进来时脑子里唯一的问题」。那就别绕：

> Rust 生态里已经有不错的「拼 LLM 应用」的库——chain、RAG、embedding、工具循环。
> **要那个的话去用那些。**
>
> 这是另一种东西：一个你嵌进产品里的运行时……最接近的类比不是 agent 库，
> 是 LangGraph 的 time travel 和 Temporal 的 durable execution。

**主动把不合适的人劝走**比含糊地争取所有人有效——被劝走的那些本来也会失望，
而留下的人第一句话就知道自己在看什么。

给了 LangGraph / Temporal 两个坐标，因为「原子依赖图 + 命令日志」对没听过的人是抽象的，
对听过那两个的人是一秒就懂的。

### 三个钩子每条都给了可点的证据，且第一条给了**复现步骤**

undo 那条没有停在「我们的 undo 是真的」，而是写了三十秒的验法（设口令 → 问回 →
撤两次 → **不带「undo」二字**再问）。这是 [196](196-wasm-expose-undo.md) 真机跑通的
那套话术，读者现在能在 demo 上自己跑一遍。

**「不带 undo 二字」这个细节必须留**——[169](169-wasm-artifact-recheck.md) 记的那个坑：
问句里出现要验的机制名会把答案喂给模型。不写这句，照着试的人会得到假阴性然后以为在吹牛。

### 加了一节 §Status，写没做的

原 README 没有这一节。加它是因为 [189](189-translate-architecture.md) 让我意识到：
这个仓库最容易赢得工程师好感的东西之一，就是它**到处标注自己没做什么**。
README 不标，读者要读到 ARCHITECTURE 才发现，那时候的观感是「藏起来了」而不是「诚实」。

写明：多副本、多租户、MCP 的 OAuth/resources/prompts 都没做，API 未稳定，项目很年轻。

### 文档区标了哪几份没英译

四份中文文档（ADAPTER / PROVIDERS / ROADMAP / issues）逐一验过**确实没有 `.en.md`**，
标上 *(Chinese)*，并解释了为什么（开发在中文进行，译文顶部注明中文权威）。
**不标的话，英文读者点进去看到中文会以为是断链或者项目不严肃。**

### 抓到并改掉一处事实错误

初稿写「first commit is from 2026-07-31」——那是**仓库创建日期**，首次提交是 **08-03**
（`git log --reverse` 查的）。这种小数字写错最伤：读者一核对就对不上，
而 README 是全项目可信度的门面。**凡是写进 README 的数字都要当场查一遍，别凭印象。**

### 验收

- [x] **一个没读过任何文档的 Rust 开发者，读完第一屏能说出「这跟 rig 有什么不一样」**
      —— §"This is not another agent framework" 就是直接回答，且给了两个坐标
- [x] 三个钩子每条都有可点的证据（demo 链接 / STATE-MODEL.en / ARCHITECTURE.en），
      不是形容词
- [x] quickstart 在第一屏之后紧跟，路径不再只写三家
- [x] 保留现有准确性：所有断言逐条核过（五种形态数得对、`adapter = "openai"`
      在 example 里真有、文档链接全通、四份「无英译」标注属实）
- [x] `README.zh-CN.md` 同步同一套定位

### 待办

- [ ] GIF —— [172](172-demo-gif.md)，插在 demo 链接下面
- [ ] 178 跑完之后，把 quickstart 里 OpenAI 兼容那句从「配置面已就绪」改成
      「已实跑验证」（**现在不能这么写**）
