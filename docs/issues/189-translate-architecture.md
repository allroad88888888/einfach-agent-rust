# 189 英译 `ARCHITECTURE.md`

**里程碑** L · **依赖** [188](188-translate-invariants.md)（沿用术语表） · **模型** sonnet · **估时** 20min · **状态** 完成（2026-08-13）

## 目标

362 行。这是**想认真评估这个项目的人**会读的第二份文档（第一份是 README）。
它译不出来，「可嵌入」这个卖点就只对中文读者成立。

## 做什么

`docs/ARCHITECTURE.en.md`，规矩同 [188](188-translate-invariants.md)（并存、中文权威、
顶部注明）。术语沿用 188 建的对照表。

重点译准的段落：

1. **包结构与依赖方向**——十个 crate 各管什么、为什么依赖方向是那样。
   这段决定读者能不能判断「我的宿主该嵌哪一层」
2. **部署形态**（CLI / 独立 server / 桌面 / wasm / Java 网关）——[165](165-launch-positioning-decision.md)
   的第三个钩子就在这里，是差异化的核心
3. **协议类型由 ts-rs 生成，不手写**——工程细节，但很能建立信任
4. §多副本那段**标注清楚是草案、未实现**。原文写了，译文别弄丢——
   这种「诚实标注未完成」的地方是可信度的来源，不是要藏的短处

## 验收

- 三处部署形态的差异译得让人能自己判断该用哪种
- 术语与 [188](188-translate-invariants.md) 的对照表一致
- 「草案 / 未实现」的标注一处不漏
- 内部文档链接指向对应的英文版（有的话）或中文版（没有的话），**不要留死链**

---

## 实做记录（2026-08-13）

`docs/ARCHITECTURE.en.md`。规矩同 [188](188-translate-invariants.md)：并存、中文权威、
顶部注明，并显式说明术语沿用 [INVARIANTS.en.md](../INVARIANTS.en.md) 底部那张表。

### 验收第三条是这份的重点，机械核对过

「§多副本那段**标注清楚是草案、未实现**。原文写了，译文别弄丢——这种『诚实标注未完成』
的地方是可信度的来源，不是要藏的短处。」

中文版共**四处**这类标注，逐处核对全部译到：

| 处 | 中文 | 英文 |
|---|---|---|
| §多副本粘性路由 标题 | **设计草案，未实现** | **design sketch, not implemented** |
| 同节 §现状 | 「下面整节一行代码都没有」 | "there is not one line of code behind this section" |
| §边缘无关 身份 | **未实现，未排期** | **not implemented, not scheduled** |
| §桌面版 `desk:` 工具 | 「一个都没注册」 | "not one `desk:` tool is registered" |

其中两处的语气特别值得保：

- 多副本那段的「**别照这节做容量规划**」译成 "don't do capacity planning from this
  section"——这句是给读者的**行动警告**，不是免责声明，译软了就废了
- 身份那段的「原稿写的『字段现在就留着』**是一句反向承诺**」译成 "is a promise in
  reverse"，并保留了后半句「照原话做多租户规划会踩空」

### 结构核对

`##` 10/10、`###` 5/5，章节一节不缺。链接用脚本逐条 resolve 过（不是 grep），全通。

### 几处译得比较费思量的

- **「位置决定语义」**（ring 在 HTTP 层不在 actor 里）→ "Location determines semantics"，
  并把后面那句「这是形态的诚实边界，不是缺陷」保成
  "an honest boundary of this shape, not a defect"
- **「红线 6 就成了自证」**（客户端能伪造 epoch 的话）→ "red line 6 would be
  self-certifying"——这句是整段的要害，不能译成「就失效了」那种平淡说法
- **「透传不是一个功能，是不做过滤的自然结果」** → "Pass-through isn't a feature;
  it's what happens when you don't filter."
- **「别把『设计好了』当成『能用了』」** → "don't mistake 'designed' for 'working'"

### 待办

- [x] ~~[190](190-translate-state-model.md) 沿用同一张术语表~~ —— 逐条核过，
      核法与结论（含因此改掉的 `日志` 那一行）记在 [188](188-translate-invariants.md)，
      不重复。**本份是唯一被核出问题的**：`日志` 在这份里有两个义项——
      命令日志与普通日志——英译分别是 *command log* 与 *logs*，分得对，
      但术语表当时只写了一个义项。
- [x] ~~中文版更新时这份会滞后~~ —— 定了：顶部钉住译自哪个 commit（本份 `5e45a2a`）
      + 一条算滞后的 `git log` 命令。取舍见 [190](190-translate-state-model.md)。

      顺带一个刚好在这次显出来的性质：**上面改术语表动的是英译，
      而 hash 记的是中文源的 commit——改译文不会让标记失效。**
      标记盯的是「源动了没有」，这正是它该盯的东西。
