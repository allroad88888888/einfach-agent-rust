# 191 首发帖：Show HN / r/rust 的文案与时机

**里程碑** L · **依赖** [179](179-readme-rewrite.md) + [183](183-post-providers.md) · **模型** **opus** · **估时** 20min · **状态** 待开始

## 目标

首发只有一次。前面所有 issue 都是为这一刻做的准备。

**用 opus**：文案、渠道、时机三件事都是判断活，而且**没有第二次机会**——
Show HN 同一个项目再发一次效果会差很多。

## 前置检查（一条不满足就别发）

- [ ] demo 链接可用且**当天验过**（[170](170-pages-workflow.md)）
- [ ] README 第一屏有 demo + GIF（[173](173-readme-demo-hero.md)）
- [ ] 有一条零成本试用路径（[177](177-openai-compat-config.md) 的 Ollama）
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
