# 167 修 README 里已被删除的 skill 激活机制

**里程碑** L · **依赖** [165](165-launch-positioning-decision.md) · **谁做** claude · **状态** 完成（2026-08-13）

## 目标

README 的**头条卖点**描述的是一个**已经不存在的子系统**。技术型读者读完 README 再读代码，
第一反应是「文档不可信」——而这个项目全部说服力都建立在「文档和代码一致」上，
这一处比没有 README 更伤。

## 现状

两份 README 里的过期描述：

- `README.md` 那张流程图：`compact skill index → **AI activates one bundle** →
  instructions + tools appear`
- `README.md` §Large capability catalogs stay lazy：「It explicitly **activates** the
  relevant skill before the full instructions and its tool schemas enter the request」
- `README.zh-CN.md`：「只有 AI 主动**激活**某个 skill 后，它的完整 instructions 和
  tool schemas 才进入请求」

**决策 27（M15）把这套整条删了**（[141](141-remove-activation-subsystem.md) 落地）：
`skills_active` / activate / deactivate / `late_system` 注入全部不存在，
`Slot::SkillsActive` 只留壳给老快照反序列化。

今天的真实形状（权威在 [../TOOLS.md](../TOOLS.md) §「今天的形状」）：

```
srv:skill/index   SessionStart 时机工具，建会话那一刻跑一次，结果进 prefix_chunks
srv:skill/read    普通工具，模型按 id 现取正文，正文经 tool_result 进【对话消息】
```

关键区别：**正文不进 system 段**，走消息尾部追加。

## 做了什么

1. 流程图重画成六跳的真实路径（声明 → 开局索引进前缀 → 按需 read → tool result 进对话 →
   宿主执行工具 → 结果续轮）。
2. 「activates」那段改写：索引常驻 + 正文按需读 + **不往 system 段中途注入任何东西**。
3. **顺手升了一级**（超出原计划）：把 [143](143-m15-dogfood.md) 的真机数字写进 README——
   DeepSeek 十轮 cached/prompt **97.5%–99.8%，均值 98.5%，含发生正文读取的那几轮**。
   原计划只是「把过期描述改对」，加上这个数字后，这一段从**解释一个机制**变成
   **亮一个别人没有的证据**。对 [165](165-launch-positioning-decision.md) L1 的英文技术读者
   来说这是完全不同量级的说服力，且它本来就是真的、只是没写出来。
4. `README.zh-CN.md` 同一段同步改掉。

## 验收

- [x] `grep -i activat README.md` 零命中
- [x] `grep 激活 README.zh-CN.md` 零命中
- [x] 改写后的描述与 `docs/TOOLS.md` §「今天的形状」一致（两个工具名、两个时机、
      「正文不进 system 段」这三点逐条对得上）
- [x] 引用的缓存数字与 [143](143-m15-dogfood.md) 实做记录一致，没有四舍五入夸大

## 顺带核实

`docs/ARCHITECTURE.md` 里没有残留的 activate 描述（`grep -i activat` 零命中），
这次不用动它。
