# 075 工具表自己不判重：同名 spec 两条都进 prompt，可逆性却只留最后一条

**里程碑** M10 期间捞到 · **依赖** 064（它要拆 `tool_table.rs`，别撞） · **模型** sonnet · **独测** ✅（红线 11 + 可逆性静默错档）

069 拍板撞名策略时列的代码清单第 1 条，**兜底那一层**。074 已经把 MCP 那一跳（`list_tools`
同 server 内重名）堵上了，本 issue 堵的是**工具表自己**——防的是别的来源、以及将来新增的来源。

## 现象：同一批数据，两种容器，两种撞名语义

`ToolTable` 的 `with_*` 系列（`with_mcp`、`with_host_tools`）对同一批 `(spec, reversibility)`
做两件事：

| 去哪 | 容器 | 撞名时 |
|---|---|---|
| spec | `Vec::push` | **两条都留** |
| 可逆性 | `BTreeMap::insert` | **只留最后一条** |

两个容器各自都没错，**合起来**就是：模型看第一份说明书、`snapshot()` 的可逆性用第二份。
`/undo` 屏障因此可能按错的档办——不报错，只在撞上它的时候以静默错值浮出来
（红线 6/11 那一类）。

## 为什么还要做（074 不是已经堵了吗）

074 堵的是**一个具体来源**（MCP 的 `tools/list`）。069 的判据是「在最早能报给有权修它的人的
那个点上失败」，那一跳因此必须有；但**表这一层是最后一道**，它防的是：

- 069 复查过的四条路之外、**将来新增的第五条**（本仓一年内加了 MCP、skill、宿主注入三条）；
- 同一条路上**绕过 `list_tools`** 的装配方式（`with_mcp` 是 `pub`，谁都能调）；
- 069 已经证实的一条事实：**当前五档 + CLI 的装配链没有实际撞名**（有测试逐张点名跑过），
  所以这一层今天**不会改变任何既有组合的行为**——它是护栏，不是修 bug。

## 范围

1. `with_*` 系列里的 `self.specs.push(spec)` 收进一个私有 `push_spec`：**名字已经在表里 →
   整条丢弃**（spec 不 push、可逆性也**不 insert**），并 `debug_assert!` 点名是谁。
2. **丢的是「后来的」那一条**——与 069 给的方向一致（只加不改，工具表在 prompt 最前面，
   红线 11）。
3. **生产不硬失败**（069 明确否决）：`with_mcp` 收的是第三方 server 的回包、
   `with_host_tools` 收的是客户端请求体，让外部把宿主进程打死是不可接受的。
   `debug_assert!` 只在 debug 构建炸，release 静默丢弃——**但看门狗测试要钉住行为**。
4. **不要**顺手改 `skill_injection` 的 `late_tools` 过滤（那是 **064** 的第 5 条，069 已分派）。

## 验收（可判定）

- **重复装载同一个工具** → `specs()` 长度不变、**可逆性仍是先来的那份**
  （两次的可逆性故意写得不一样，否则「留哪条」不可判定——074 的先例）。
- **`debug_assert!` 点得出名字**：断言 panic 消息里含那个工具名。
- **不误伤**：名字不同的照常全进；069 那条看门狗
  （`crates/agent-runtime/tests/tool_table_names_are_unique.rs`）继续绿。
- **突变验证（必须做，贴真实红/绿输出）**：把判重那段删掉 → 上面第一条红 → 改回 → 绿。

## 注意

- ⚠️ **等 064 落地再做**：064 要给 `skill_injection` 加过滤，而 `tool_table.rs` 当时
  296 行、加东西必顶破 300，**它会按职责拆这个文件**。本 issue 在拆完的结构上做，
  别两边同时拆。
- 红线 11：本 issue 直接关系到「进 prompt 的表」，063 刚落的两个字节确定性测试
  （`host_tools_prefix_is_byte_deterministic.rs`、`host_tools_prefix_head_never_moves.rs`）
  是你的安全网，**跑绿它们**。
- 红线 9：≤300 行。
- 收工验证前台跑完（WORKFLOW §四 -1），含 `--features ts`（收工清单那一条）。

## 实做记录（完成 · 2026-08-04）

**`push_spec` 长什么样**（`crates/agent-runtime/src/tool_table.rs`，私有方法，`from_specs`
之后、`builtin()` 之前）：

```rust
fn push_spec(&mut self, spec: ToolSpec) -> bool {
    if self.declares(&spec.name) {
        debug_assert!(false, "ToolTable 已经有工具 `{}` 了，同名的后来这一条整条丢弃（specs 不 push，可逆性也不 insert）", spec.name);
        return false;
    }
    self.specs.push(spec);
    true
}
```

返回值交给调用方决定要不要顺带 `insert` 可逆性映射——`with_mcp`（`tool_table.rs`）和
`with_host_tools`（`tool_table_host.rs`）都改成 `if self.push_spec(spec) { self.xxx_reversibility.insert(name, reversibility); }`；没有旁挂映射的 `with_spawn`/`with_status`/`with_collect`（`tool_table.rs`）和
`with_skills`（`tool_table_skill.rs`，两次 `push_spec(activate_spec())`/`push_spec(deactivate_spec())`）
直接把 `self.specs.push(x)` 换成 `self.push_spec(x)`。判重覆盖了**全部**六个 `with_*` 系列
里 `self.specs.push` 的调用点，不只是 069 §拍板 D 点名的 `with_mcp`/`with_host_tools` 两条
——`push_spec` 是唯一入口，五档 CLI 装配链和 skill 装配也自动获得同一份保护（`with_skills`
被连续调用两次这种编程错误现在也会被 CI 抓住）。

### 既有行为是否真的一个字节没变——是

- `crates/agent-runtime/tests/tool_table_names_are_unique.rs`（069 的看门狗，逐张点名五档
  + CLI 链）改动前后全绿，两条断言、断言内容一个字不改。
- `host_tools_prefix_is_byte_deterministic.rs`、`host_tools_prefix_head_never_moves.rs`
  （063 的字节确定性安全网）全绿。
- `tool_table_skill_tests.rs`（064 的 `late_tools` 过滤）五条既有测试全绿，没有碰
  `skill_injection` 里那行 `retain`。
- `cargo test -p agent-core -p agent-server -p agent-runtime`：536 passed / 0 failed
  （85 个测试二进制）；`cargo test -p agent-server --features ts`：全绿，`EXIT: 0`；
  `cargo clippy --workspace --all-targets -- -D warnings`：0 条 warning/error；
  `scripts/check-invariants.sh --all`：`红线检查通过`。这四条都比任务书给的基线
  （1501 passed）只多不少（新增了 8 条 075 自己的测试）。

**一处被迫改写的既有测试，不是行为回归**：`tool_table_host_tests.rs` 里
`the_injection_map_wins_over_the_mcp_map` 原来专门构造过一次跨域撞名
（`with_mcp` 装一个 `mcp:everything/echo`，再用 `with_host_tools` 塞一个同名但可逆性不同
的），断言「注入映射赢」。这条断言本身就是 069 §现状描述的那个 bug 的产物——`specs`
两条都留、`host_reversibility.insert` 后来居上，`snapshot` 先查 host 表所以看着像
「赢」，但表里其实同时躺着两条同名 spec。`push_spec` 接管之后这个构造从「静默产生两条
spec」变成「debug_assert 炸」，旧断言不再成立，因为它测的正是被 075 判定为违规的那个
中间状态。这不是「捞到一条真 bug 后默默放过」，而是把它从「用一个巧合行为当断言」改写
成「显式钉住新行为」（重命名注释说明原委，并新增
`a_name_that_collides_across_with_mcp_and_with_host_tools_keeps_the_first_one_registered`
把「跨 with_* 边界撞名同样被拦」钉成正式断言）——**没有任何生产装配路径依赖过旧断言的
那个中间状态**（069 已证实五档 + CLI 链不撞名，这个场景本来就只在直接调库 API 时才够得着，
`with_host_tools` 的名字前缀在 HTTP 层被 061 强制成 `web:`/`desk:`，`mcp:` 前缀的名字
结构上进不去）。

### 突变验证（真实红/绿）

把 `push_spec` 里的判重整段删掉，只留 `self.specs.push(spec); true`，跑
`cargo test -p agent-runtime --lib tool_table`：

```
running 37 tests
...
failures:
    tool_table::host::tests::a_name_that_collides_across_with_mcp_and_with_host_tools_keeps_the_first_one_registered
    tool_table::host::tests::with_host_tools_loading_the_same_name_twice_keeps_the_first_reversibility
    tool_table::host::tests::with_host_tools_names_the_offender_in_a_debug_build
    tool_table::skill_tools::tests::with_skills_called_twice_does_not_duplicate_the_activate_and_deactivate_tools
    tool_table::skill_tools::tests::with_skills_names_the_offender_in_a_debug_build
    tool_table::tests::push_spec_leaves_specs_untouched_when_the_name_already_exists
    tool_table::tests::with_mcp_loading_the_same_name_twice_keeps_the_first_reversibility
    tool_table::tests::with_mcp_names_the_offender_in_a_debug_build

test result: FAILED. 29 passed; 8 failed; 0 ignored; 0 measured; 75 filtered out; finished in 0.00s
```

八条新测试全部按预期变红（覆盖 `push_spec` 本身、`with_mcp`、`with_host_tools`、
`with_skills`、跨 `with_*` 边界四个层面），其余 29 条不涉及判重的既有测试维持绿——
交叉验证「不误伤」。改回判重逻辑后重跑同一条命令：`test result: ok. 37 passed; 0 failed`。
`grep -rn "MUTATION" crates/` 确认改动已清干净。

### 为什么每条新测试都要 `catch_unwind` + `cfg!(debug_assertions)` 分支

`debug_assert!(false, ...)` 在 debug 构建（`cargo test` 默认）下遇到判重分支必炸，且
`with_mcp`/`with_host_tools`/`with_skills` 按值消费 `self`——一炸整个调用的返回值就没了，
没法在同一次调用里既拿到「它确实炸了」又拿到「炸之后 specs 长度不变、可逆性是先来的
那份」。所以每条验收测试对整个构建结果 `catch_unwind`：debug 分支钉住「确实按 069 §拍板
D 的要求炸了，且消息点得出名字」（`push_spec_leaves_specs_untouched_when_the_name_already_exists`
额外验证了 `push_spec` 本身在 debug 下抛出的 panic 之前状态未变——它只借用 `&mut self`，
不消费，所以能在 catch_unwind 之后照样读 `table`）；release 分支钉住验收要求的「specs
长度不变、可逆性仍是先来的那份」。四个 `#[should_panic(expected = "<name>")]` 测试独立
覆盖「debug_assert 点得出名字」这一条（`mcp:dup/tool`、`web:dup/tool`、
`srv:skill/activate`，加上 `push_spec_leaves_specs_untouched_...` 里的 `srv:fs/read`）。

### 有没有顺手捞到真 bug——没有

没有发现任何既有装配路径因判重丢工具；`push_spec` 覆盖的六个调用点里，唯一因新逻辑而
必须改写的旧测试（`the_injection_map_wins_over_the_mcp_map`）本身构造的就是一个此前
"能通过但依据是 bug"的场景，处理方式见上，不构成需要单独报告的回归。
