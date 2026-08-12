# 153 `TimedRun` 加只读 `&Session`，ext:stats 删传话格 ← M16 终点

**里程碑** M16 · **依赖** [150](150-derived-extension-decision.md)（已拍板 = 决策 30） · **模型** sonnet · **独测** — · **状态** 待做

## 目标

决策 30 的唯一实现刀：`TimedRun` 签名加**只读** `&Session`——两个时机驱动
（`run_session_start` / `turn_end::fire`）手里本来就攥着 session，递进去而已。
149 里 ext:stats 被迫发明的内存传话格（`Ledger` + `seen_at` 标注）随之整个删除：
审计行改为轮末**现读**稳定态。

## 做什么

1. `TimedRun` 签名：`Fn(&ToolTable, &Session, &Value) -> Result<Arc<str>, Arc<str>>`
   （`&Session` 放中间，紧挨它要读的世界；只读——**谁要 `&mut` 谁去写截获工具**，
   类型即边界，v1 的「hook 不写状态」从纪律变成签名）。
2. 两个驱动递 session；`turn_end::fire` 的调用点签名跟上。
3. 所有既有 timed 执行体跟签名：skills 索引闭包（`with_skills`，参数忽略即可）、
   ext:stats 的 hook、`ExtensionPack::with_timed` 的文档与测试 fakes、各独测里的
   fake timed 工具（146/148/145 的独测文件里签名要改——**只改签名不改断言**，
   属于公开类型演进的机械跟随，报告里逐个列出）。
4. ext:stats：删 `Ledger` 传话格与 `seen_at`；审计行改为
   `turn=N entries=X/Y agents=Z tools=W`（数字来自轮末现读）；相应单测改写；
   `docs/EXTENSIONS.md` §五 教材同步（传话格那段整段删除，换成一句
   「hook 拿只读 Session 现读」）。
5. 149 实做记录里的审计样例**不改**（那是当时的真机事实）；本条实做记录注明
   格式自此变化。

## 验收

- 全部既有 timed 相关测试改签名后全绿；skills 索引闭包行为**逐字节不变**。
- ext:stats 审计行含轮末实读数字（新单测：跑两个完成轮，第二行的 entries
  等于当时 `history_len`）。
- hook 无法写状态：`&Session` 不可变，编译即证明（不需要测试）。
- `cargo test --workspace` 全绿 + `check-invariants` 过 + `build-wasm` 绿
  （TimedRun 是公开类型，wasm 侧编译要验）。

## 注意

- 公开类型签名变更（红线 11 不涉——签名不进 prompt），但独测文件要动：
  这是「公开类型演进的机械跟随」，不是改断言——diff 里独测文件只许出现
  闭包参数列表的变化。
- 别顺手给 SessionStart 的执行体开写口或加别的参数——决策 30 只批了只读这一刀。
