# 133 工具表加「调用时机」维度

**里程碑** M15 · **依赖** — · **模型** sonnet · **独测** ✅ · **状态** 完成（2026-08-11）

## 目标

`ToolTable` 长出第三个正交维度 `CallTiming`：**空 = 模型自主调**（今天的全部工具）、
`SessionStart` = 会话创建时 runtime 自动调、`TurnEnd` = 每个完成轮后 runtime 自动调。
**时机非空的工具不进模型清单**——模型看不见它，也调不到它。

这是 M15 全部机制的地基（决策 27）。本条只加维度，不加任何驱动（135/136 的事），
也不装任何真工具。

## 现状

判定侧已有两个宿主现算的维度：`location_of` 按名字前缀推、`reversibility_of` 按名字
查表（`agent-runtime/src/tool_table.rs`）。specs 是 `Vec<ToolSpec>`，push 顺序即注册
顺序。`declares()` / `snapshot()` / 五条截获闸全部按名字查。

076 的 disable（`tool_table_disable.rs`）拍过一条关键判据：**「表里有什么」和「模型
看得见什么」必须是同一个答案**，`declares()` 只能回答一个——所以减法在进表那一刻
结账，不留渲染期过滤。timed 工具必须遵守同一条。

## 做什么

1. `CallTiming { SessionStart, TurnEnd }` 定义在 `agent-runtime`（core 不需要认识
   时机——它连「工具由谁发起」都不该知道，红线 12 的精神）。
2. timed 工具住**独立区**，不混进 specs `Vec`：`Vec<(ToolSpec, CallTiming)>` 之类，
   保 push 顺序。于是 `specs()`（喂模型的那份）与 `declares()` 天然不含它们——
   模型硬猜名字发 ToolCall，走既有 `unknown_tool` 路，不需要任何新判断。
   这正是 076 那条判据的延续：模型面的表仍然只有一个答案。
3. timed 条目**自带执行体**：注册签名形如
   `with_timed(spec, timing, run: Box<dyn Fn(&ToolTable, &Value) -> Result<Arc<str>, Arc<str>> + Send + Sync>)`。
   驱动（135/136）直接调它，**不走 dispatch/executor/远端槽**。这意味着 v1 的
   时机工具**结构上只能是本地同步执行**——会话创建时 SSE 还没接上，`Web` 位置
   的开局工具永远等不到回写，与其加位置校验不如让这条路在签名上就不存在；
   要支持远端/MCP 时机工具是将来的显式扩展，不是本条的洞。
4. 查询接口：`timed(CallTiming)` 按注册顺序迭代（spec + 执行体）。
5. 撞名：timed 名与模型面名互撞 = 违反「一个名字一条执行路径」红线。作者是程序员
   （装配代码），按 069 判据落 `debug_assert!` + 看门狗测试
   （`tool_table_names_are_unique.rs` 扩一条断言覆盖 timed 区）。
6. `docs/TOOLS.md` §「模型看到的是一张扁平表」补 timing 维度一段。

## 验收

- timed 工具不出现在 `specs()`；`declares()` 为假；对同名的模型 ToolCall 走
  `unknown_tool` 路（既有行为，加断言钉住）。
- `timed(SessionStart)` 迭代顺序 == 注册顺序；测试里交换两个 timed 工具的注册顺序，
  断言迭代顺序跟着换。
- timed 条目的执行体拿 `&ToolTable` 自身可以读到 registry 等表内数据
  （138 的索引函数要用），且执行不产生 entry、不碰 dispatch。
- 既有装配组合（五档 + CLI 链）timed 区为空时，`specs()` 序列化**逐字节不变**
  （红线 11 看门狗）。
- timed 名与 specs 名相撞 → `debug_assert!` 失败；看门狗测试覆盖。

## 注意

- **红线 11**：specs 在 prompt 最前面，本条的过滤/不过滤必须是装配期一次结账，
  不是每轮渲染期判断。
- **timing 不进 `ToolSpec` 三字段**，不进协议面（`packages/protocol` 零改动）——
  模型面契约一字不动。
- 别在本条里加驱动或真工具——135/136/138 各自验收，混了回滚不了。

## 实做记录（2026-08-11）

- 落点：`tool_table_timed.rs`（126 行）+ `tool_table_timed_tests.rs`（198 行），接口与契约零偏离；
  timed 条目自带执行体（`Box<dyn Fn(&ToolTable, &Value) -> Result<Arc<str>, Arc<str>> + Send + Sync>`）。
- 撞名双向拦（with_timed 查两边 + push_spec 反向判重），看门狗测试扩到 timed 区。
- TOOLS.md 补「第三个维度：调用时机」一节。独测 `call_timing_indep.rs` 六条全绿（含
  builtin() specs 序列化逐字节不变的红线 11 断言）。
