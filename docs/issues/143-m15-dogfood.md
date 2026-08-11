# 143 真机 dogfood ← M15 终点

**里程碑** M15 · **依赖** [136](136-turn-end-driver.md) + [141](141-remove-activation-subsystem.md) + [142](142-skill-hidden-frontmatter.md) · **模型** **opus** · **独测** 本条即验收 · **状态** 待做

## 目标

M15 的全链真机验收（真 provider，不是 mock），跨 133–142 的交界处走一遍。
132 的先例：dogfood 专门抓「每条 issue 各自绿了、合起来漏的那种」。

## 准备

skills 目录：一个 router skill（正文引用一个 hidden 子 skill 的 id）+
该 hidden 子 skill（正文藏口令）。一个 fake `TurnEnd` hook 工具（计数落临时文件）。

## 验收（逐条可判定，全过才算 M15 完成）

CLI + DeepSeek：

1. 首轮 system 含索引（来自开局工具的前缀块）、含 router 不含 hidden 子 skill 的
   行、**不含任何正文字节**。
2. 问口令：模型自主 read router → 顺正文引用 read hidden 子 skill → 说出口令。
   **两跳都是模型自己决定的**，没有任何预激活。
3. undo 撤掉含 read 的那轮 → 下一轮 encode body 中正文字节消失
   （撤回并进 undo，决策 27 的承诺）。
4. `kill -9` 重启恢复：前缀块逐字节原样；开局工具执行计数**仍是 1**（不重跑）。
5. 十轮里每个完成轮 hook 恰好触发 1 次；`Ctrl-C` 取消的那轮 0 次。
6. 十轮第 2 轮起每轮 `cached_tokens / prompt_tokens ≥ 0.9`——含 read 发生的
   那几轮（正文是消息尾追加，不该破前缀）。

server（宿主声明路）：

7. `capabilities.skills` 声明一个 skill → 索引含它 → read 取到 → 杀进程恢复后
   再 read 仍取到。

## 回填

- 逐条兑现记录写进本文件；发现的漏投影/交界 bug 各自开小 issue 或就地修
  （132 的先例：能一行修的就地修并记录）。
- `ROADMAP.md` §二加 M15 完成段；§四「工具索引 + 详情按需拿」条目下确认
  M15 已落的那一半的真机数字（第 6 条的命中率）。
- `CLAUDE.md` 当前状态段该更就更。

## 注意

- 验收 6 失败的第一嫌疑不是代码是措辞：read 太频繁（每轮都读同一个 skill）
  说明模型没意识到正文已在历史里——先调 read 的 description 与索引首行文案。
- 花真钱：单 agent 跑，别并发派两个（WORKFLOW §四 -2 的教训）。
