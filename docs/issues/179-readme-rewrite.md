# 179 README 重写（英文定稿）

**里程碑** L · **依赖** [173](173-readme-demo-hero.md) + [178](178-openai-compat-dogfood.md) · **模型** **opus** · **估时** 20min · **状态** 待开始

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
