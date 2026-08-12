# 145 spawn 入参 `inherit_prefix` + `system_for` 过滤

**里程碑** M15 追加（决策 28） · **依赖** [144](144-prefix-allowed-slot.md) · **模型** sonnet · **独测** ✅ · **状态** 待做

## 目标

决策 28 的模型面闭环：`srv:agent/spawn` 收可选字符串数组 `inherit_prefix`，
从严校验后随 spawn 快照落 144 的槽位；`system_for` 按名单过滤前缀块。
**缺省 = 全带 = 今天的行为**——143 的真机结论不受影响。

## 现状

- spawn 入参解析在 `spawn_request.rs`（task/tools/background 三字段，
  形状校验的错误文案风格照它既有的）。
- 前缀块 label 格式 `init:<工具全名>` 是 135 钉的契约（`session_start.rs`）。
- `system_for`（`subagent.rs`）现在无条件追加 `session.prefix_chunks()`。
- timed 区名集：`ToolTable::timed(CallTiming::SessionStart)`。

## 做什么

1. `spawn_request.rs`：解析 `inherit_prefix`（缺省 `None`；非字符串数组 →
   既有风格的错误文案）；schema `properties` 加字段，description 写清三档语义
   （「缺省全带；`[]` 全不带；列 timed 工具名挑着带——子任务不需要会话开局材料
   （如技能索引）时用它省上下文」）。
2. 名字校验**从严**：每一项必须 ∈ `timed(SessionStart)` 的 spec 名集，否则
   `is_error` 的 tool_result、**整次 spawn 不发生**，文案点名非法项（决策 20
   兜底同款：让模型自纠，不静默跳过）。校验在 `spawn_tool::intercept`（表在
   ctx 手上）。
3. 校验通过的值传给 `Session::spawn_child`（144 的新参数）。
4. `system_for`：追加前缀块前查 `prefix_allowed_of`——`None` → 全带；
   `Some(set)` → 只带 label 为 `init:<name>` 且 `<name> ∈ set` 的块
   （label 前缀约定写注释，出处指向 135）。
5. **补一条看门狗**（此前讨论确认的测试空白）：spawn 两个子 agent 并跑完一轮后，
   开局工具执行计数仍 = 1——「子 agent 不重跑 SessionStart」从结构性论证升级为
   会红的断言。

## 验收

- 缺省（不填字段）：子 system 含索引块，与 145 落地前**逐字节相同**（红线 11
  向后兼容，也是 143 结论继续有效的依据）。
- `[]`：子 system 无任何 `init:` 块；同参数姊妹子 agent 前缀逐字节相同
  （缓存共享性质保住）。
- `["srv:skill/index"]`：带；`["srv:skill/nope"]`：`is_error`、无子出生、
  父轮继续。
- 线级（fake provider）：同一轮两个子，一个缺省一个 `[]`——各自请求体的
  system 段有/无索引文本。
- 恢复后过滤依旧（值从 144 的状态来，不重算）。
- 看门狗：spawn 两子跑完一轮，开局工具执行计数 = 1。
- spawn spec 的 description 含 `inherit_prefix` 说明（spec 断言钉住）。

## 注意

- **红线 11**：spawn description 变更 = 新会话工具表字节变化，如实且只影响
  新会话；**缺省语义不变**是本条的硬约束，动了它 143 的验收就得重跑。
- 字段只管 `SessionStart` 产物——`TurnEnd` 没有 per-child 语义（子不跨 turn，
  没有轮末），schema description 别把话说大。
- 决策 28 的取舍记录（为什么否掉「混进 tools 名单」与「从工具子集推导」两案）
  在 ROADMAP §一，别在代码注释里重复整段，链接即可。
