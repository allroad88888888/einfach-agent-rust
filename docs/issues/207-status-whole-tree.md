# 207 runtime：`srv:agent/status` 放开到整棵活树

**里程碑** M20 · **依赖** [205](205-core-peek-and-inbox.md) · **模型** sonnet · **独测** ✅ · **状态** ✅ 完成（2026-08-18，见文末）

## 目标

决策 204 §一 在观测面的兑现：**`status` 的收窄从「调用者的严格后代」改成「本会话的
全部活 agent」。** 兄弟因此互相看得见，也因此拿得到 206 要用的那个 `to`。

## 做什么

### 1. 收窄改成全树

`status_tool.rs::descendants`（`:170`）今天按 `is_descendant_of(caller)` 过滤。
改成不过滤，`session.agent_tree()` 的全部节点，仍然自己 `sort_by(AgentId)`
（`:169` 那条理由不变：不借 `live_agents` 的排序承诺）。

`observe`（`:125`）那道 `is_descendant_of` 校验（`:130`）随之去掉；`id` 参数从
「必须是你的后代」改成「必须在本会话的活树上」。

**`not_a_descendant`（`:243`）整个删掉**，`not_live`（`:254`）留着——形状对但树上
没有仍然要有话说。

### 2. 调用者自己进不进列表

**进。** 今天排除自己的理由（`:163`）是「它此刻的 activity 恒等于正在跑 status，
是句废话」——在只看后代的年代成立，现在不成立了：一份**全树**清单里独独缺自己，
模型没法从这份清单里知道自己是谁、在哪一层。列出来，activity 那一格照实写。

（自己那几样真正有用的账在 [208](208-self-tool.md)，不在这里。）

### 3. 工具描述重写

今天的第一句是「看一眼**你 spawn 出来的**子 agent 此刻在干啥」。改成整棵树，
并说清它能拿来干什么——**这份清单里的 id 就是 `srv:agent/send` 的 `to`**。

`id` 参数的描述（`:81`）里那句「必须是你的后代——你看不到自己的祖先和兄弟」
是本 issue 之后最显眼的一句假话，别漏。

### 4. 拆文件（红线 9）

`status_tool.rs` 今天 278 行。改完必然顶破 300。**拆分是本次改动的一部分**，
照 200 拆 `undo_hook.rs`、照 `spawn.rs` / `despawn.rs` 的既有先例：
`status_tool.rs` 留 `spec` + `intercept` + `observe`，渲染那半段
（`render` / `activity` / `task` / `one_line` / 拒绝文本）拆去 `status_render.rs`。

## 验收

- 两个兄弟：A 调 `status` 看得到 B，**且看得到自己**（id 与 depth 正确）。
- 子调 `status` 看得到父与祖先。
- `id` 指向兄弟 → 成功，返回那一段子树。
- `id` 指向已 despawn / 已撤销的 → `not_live` 的 `is_error`，不是 panic。
- **`/undo` 撤掉一轮 spawn 之后再 `status`，那个子 agent 消失**——活性判定仍在
  `live_agents` 那一层，这个文件里没有一行代码认识「撤销」（`status_tool.rs:9` 的
  既有性质，放开之后必须仍然成立）。
- **红线 11**：同一棵树连调两次 `status`，两段正文**逐字节相同**；换一个调用者调，
  节点顺序相同（全树清单不该随调用者变序）。
- 依然**不暴露任何 agent 的消息正文**——只有 activity + task（204 没有改这条边界，
  正文仍然是 `collect` 的事）。
- 拆完两个文件都 ≤ 300 行，`check-invariants --all` 过。
- `cargo test --workspace` 全绿 + `build-wasm.sh` 绿。

## 注意

- **`status` 走的是 `session.agent_tree()`，不是 205 的 `peek_agent`。** `agent_tree()`
  本来就是一份全树的纯派生快照（046），它从来不受红线 10 约束——受约束的是
  `status` 自己那道人为收窄。这条 issue 删的是那道收窄，**不是**改 core。
  `peek_agent` / `read_agent` 是给按槽位横读的场景留的口（`await` 在
  [212](212-await-tool-and-wait-graph.md) 用 `read_agent`），本 issue 用不上它们，
  **别为了「用一下新 API」硬改**。
- `task` 的截断与压平（`:221` / `:235`）一个字不动。全树之后正文更长了，
  `TASK_CHARS = 100` 是不是还合适**先别猜**——等 210 真机 dogfood 看实际长度再说。
- 别顺手把 `collect` 也放开。`collect` 是**领取**不是观测：领谁的结果关系到「一份结果
  只能领一次」的记账，放开它要重新算账，而 204 没有拍这件事。

## 实做记录（2026-08-18）

`cargo test --workspace` 2190 passed / 0 failed；`check-invariants --all` 退出码 0。

### 拆了五个文件，红线 9 提示 13 → 12

`status_tool.rs` 278 → 173（收窄与拒绝的**判定**）、`status_render.rs` 120（**渲染**，新）、
`status_tool_tests.rs` 392 → 268、`status_render_tests.rs` 140（新）、
`status_spec_tests.rs` 51（新——工具说明书是一段每轮都进 prompt 的字符串，
它自己就是一份要守的契约，跟收窄判定不是一件事）。

`status_tool_tests.rs` 那 392 行**本来就在基线的红线 9 名单上**，所以这次不是
「与基线逐条相同」，是少了一条。

### 逮到一个假绿灯

`status_indep_only_descendants` 原来断言 `!body.contains("right branch")`，
而兄弟的 task 恰好是 `"TASKBRIGHT work the right branch"`——**子串同时命中 task 和
answer**。老测试一直绿是因为那时两样都不出现；视野一放开它就现形了。改成断完整的
回答串（`"right branch answer"` 等三个）。

讽刺的是这个文件自己在 `listed_ids` 的注释里就警告过同一类陷阱
（`root/a1` 是 `root/a1/a1` 的子串）。**警告写在注释里，没写进断言里。**

### 两个端到端测试的前提被删了，翻过来用

- `status_indep_only_descendants.rs` → **`status_indep_whole_tree.rs`**。脚手架原样
  留着（让兄弟正在飞、用服务器时间窗把「读树那一刻它确实在飞」钉死），现在证明的是
  更强的一件事：**一个还在飞的兄弟也看得见**，视野就是真的树而不是「恰好都收敛了
  的那些」。同时守住**没有**被放开的那条边界：兄弟的 `task` 该出现，兄弟的回答正文
  仍然不许。
- `status_indep_rejects_non_descendant_id.rs` → **`status_indep_absent_id.rs`**。
  原来一次跑「上读祖先 + 横读兄弟」两条非法方向，现在两条都合法。改成同时钉两件事：
  兄弟的 id 通得过（且它真的活着），以及只剩「不在活树上」一种拒绝、被拒之后 loop
  照常往下走。

### 一处欠账留给 206

`status_spec_tests.rs` 里断言描述提到 send 用的是**字面量** `"srv:agent/send"`，
因为 `crate::SEND_TOOL` 常量还不存在。206 落地后换成常量——那样「send 改了名而这段
描述没跟上」也一样红（照同一个文件里 `COLLECT_TOOL` 那条的写法）。文件里有注释标着。
