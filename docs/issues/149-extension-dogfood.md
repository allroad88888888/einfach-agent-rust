# 149 扩展真机 dogfood：一个真扩展包走全程 ← M16 前半终点

**里程碑** M16 · **依赖** [147](147-migrate-intercepts.md) + [148](148-extension-pack-seam.md) · **模型** **opus** · **独测** 本条即验收 · **状态** 待做

## 目标

用 146–148 的机制写一个**真扩展包**并真机走全程（真 provider），验证
「第三方 Rust 扩展」这条路从写包到装配到模型使用到 undo 的完整闭环。
132/143 的先例：dogfood 专抓「各条 issue 各自绿、合起来漏」的缝。

## 扩展包内容（示例但要真有用）

`ext:stats` 包，两件东西：

1. **`ext:stats/report`**（截获式，Pure）：读 `agent_tree()` + 各 agent 的
   `messages_of` + entry label 序列，给模型吐一份「本会话至今：几轮、几次
   工具调用、几个子 agent、undo 过几次」的文本汇总。
2. **TurnEnd hook**：每完成轮把「轮号 + entry 数」追加进一个本地审计文件
   （宿主侧文件，不进状态）。

落点：`agent-cli` 内一个 feature 门或 `--ext-stats` 开关后的模块（实现者
选最小侵入的一种并记录理由）；**不新开 crate**——第一个扩展包先证明接缝，
包的独立发布形态等真有第三方再说。

## 验收（逐条可判定）

1. CLI 真机（DeepSeek）：问「这个会话到目前为止干了什么」→ 模型自主调
   `ext:stats/report` 并用返回内容回答。
2. **undo 的活演示**（这条是账本卖点的正面戏）：spawn 一个子 agent 干点活 →
   调 report 记下数字 → `/undo` 撤掉那轮 → 再调 report → **数字跟着回退**
   （树少一个节点、entry 数回落）——扩展读到的世界与账本严格一致。
3. TurnEnd 审计文件每完成轮恰好多一行；取消轮不多。
4. 不装包的会话：specs/prompt 逐字节与 M16 之前相同。
5. 十轮 `cached/prompt ≥ 0.9` 照旧（扩展工具结果走消息尾，不破前缀）。
6. `kill -9` 恢复后再调 report，数字与崩溃前一致（读的是恢复出的状态）。

## 回填

- 逐条兑现记录；发现的交界 bug 就地修并记录（132 先例）。
- `docs/EXTENSIONS.md` 补「写你的第一个扩展包」一节（以 ext:stats 为教材）。
- ROADMAP §二 记 M16 前半完成；150 的决策拿本条的手感当输入。

## 注意

- 花真钱：单发，不并发跑两个实验（WORKFLOW §四 -2）。
- report 的输出要过工具结果上限（决策 19，32 KiB）——长会话下自己截断，
  别指望 core 兜底截得好看。
