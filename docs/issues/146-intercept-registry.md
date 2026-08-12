# 146 截获式工具注册表：扩展工具不再改 dispatch

**里程碑** M16 · **依赖** — · **模型** sonnet · **独测** ✅ · **状态** 待做

## 目标

把「按名字截获、拿 Session 干活」从**手工 if 链**升级为**装配期注册表**：
新的截获式工具（= Rust 扩展访问状态的正门，决策 29）注册进表即可，
不再需要改 `dispatch.rs`。机制层通用——注册表不认识任何具体工具。

本条只建机制 + 用 fake 工具验证；既有五条截获的迁移在
[147](147-migrate-intercepts.md)，分开是为了本条能单独回滚。

## 现状

- `dispatch.rs` 的 `Effect::ExecuteTool` 分支里手工排着四条工具截获
  （spawn / collect / status / skill-read），各自
  `intercept(session, ctx, subtree, ...)` 签名略有出入；`Effect::Compact`
  的路由不是工具截获，**不在本条与 147 的范围**。
- 截获的既有判据全部沿用：**以工具表里有没有声明为准**（`declares()` 为假 →
  unknown_tool 路）；「一个名字一条执行路径」红线；076「模型面只有一个答案」。

## 做什么

1. 截获函数的统一签名：把泵局部状态打包成一个参数结构（形如
   `InterceptArgs<'_> { session, ctx, subtree, compactions, bus, agent, call_id, input, epoch }`
   ——对齐 `run_effect` 现有入参面），
   `type InterceptFn = Box<dyn Fn(InterceptArgs) -> Dispatched + Send + Sync>`。
2. 注册表住 **`RunnerCtx`**（builder 期注册，会话期不变），不进 `ToolTable`——
   表管「进 prompt 的声明 + 宿主判定数据」，执行体跟 executor 一样住 ctx
   （timed 工具的执行体在表里是例外，因为 `run_session_start` 跑在 ctx 建成
   之前、手上只有表——这条对比写进文档注释）。
3. dispatch 在既有 if 链**之后**、快照/屏障段之前查表：命中 → 调注册的闭包；
   未命中 → 照旧走后面的路。既有五条一行不动。
4. 撞名判据：注册名必须 `declares()` 为真（截获是执行路径，spec 是声明，
   一名一路缺一不可），否则 `debug_assert!` + release 忽略（照 push_spec 哲学）；
   同名重复注册同判。
5. 纪律入文档（机制不强制，如实写）：截获闭包读状态应照 `status_tool`
   的先例**按调用者的后代收窄**（红线 10 别被扩展变成横读后门）；
   写状态只走 `Session` 的 command 面。

## 验收

- fake 截获工具（测试注册）：模型脚本化调它 → 闭包收到正确的 agent/input/epoch，
  能读 `session.agent_tree()`、能经 command 写出一条带 label 的 entry，
  结果以 tool_result 回到下一轮 prompt。
- 未注册的名字行为与本条之前逐字节相同（unknown_tool 路）。
- 注册名不在表里 → debug 构建 panic；同名二次注册 → 同判。
- 既有五条截获与全部既有测试零变化。

## 注意

- 红线 2/10 是纪律面；「一名一路」是本条的硬断言面。
- 闭包 `Send + Sync`（actor 线程持有 ctx）。
- 别顺手迁移既有截获（147），也别做扩展包装配（148）。
