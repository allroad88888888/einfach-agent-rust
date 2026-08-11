# 139 CLI 装配切换：skills 新路上线

**里程碑** M15 · **依赖** [133](133-call-timing-field.md) + [135](135-session-start-driver.md) + [137](137-skill-read-tool.md) + [138](138-skill-index-tool.md) · **模型** sonnet · **独测** ✅ · **状态** 完成（2026-08-11）

## 目标

`agent-cli` 的 skills 形态切到新路：`with_skills` 改为注册
`srv:skill/index`（`SessionStart` 时机）+ `srv:skill/read`（普通工具），
**不再**产常驻 `INDEX_LABEL` chunk、**不再**装 activate/deactivate。
真机验收（真 provider，不是 mock）。

**只切装配，不删机制**——激活子系统的删除在 [141](141-remove-activation-subsystem.md)。
分两步的理由：本条出问题时一行 revert 就回老路；删了再回就难了。

## 现状

- `with_skills`（`tool_table_skill.rs:67`）今天装 activate/deactivate + 常驻索引。
- `skill_injection`（`:84`）每轮按激活集现算注入——切换后激活集恒空
  （activate 工具不在表里，没有写入路径），它自然注入空集，不用动。
- M5 的 068 真机验过口令实验：模型说不出**没激活**那个 skill 里藏的口令、
  说得出激活了的。新路重跑同一实验，预期变成「模型自己 read 后说得出」。

## 做什么

1. `with_skills` 重写：index 进 timed 区（`SessionStart`）、read 进 specs、
   activate/deactivate 与常驻索引 chunk 移除。
2. CLI 创建路已接 135 驱动，无需新代码——本条验证接线真的通。
3. 真机（DeepSeek）：skills 目录放一个正文藏口令的 skill，问模型口令。

## 验收

- 新会话 specs：无 `srv:skill/activate`/`deactivate`、**有** `srv:skill/read`、
  无 `srv:skill/index`（在 timed 区）。
- 首轮 encode body：system 含索引块（来自前缀块，label `init:srv:skill/index`）、
  **不含任何 skill 正文字节**；正文只出现在 read 的 tool_result 里。
- 真机口令实验：模型自主调 read 并说出口令（068 的对照组行为）。
- 真机十轮（含至少两次 read）：第 2 轮起每轮 `cached_tokens / prompt_tokens ≥ 0.9`
  ——这条是 M15 拿钱说话的核心验收，skills 路不再破前缀。
- 老会话兼容：journal 含 activate entry 的 M5 期会话恢复不 panic，
  `skill_injection` 照旧工作（141 之前的兼容态）。

## 注意

- **红线 11**：这是模型面工具表的形状变化，只影响**新会话**（表是会话创建期
  装配的）；存量会话的表一个字节不动。
- read 的 description 是模型「会不会去读」的唯一引导（决策 27 认账的代价：
  没有硬保证）——口令实验失败先调 description 措辞，别急着回退结构。
- 别顺手删 `skill_injection`/`late_system`/core 激活集——141 的事。

## 实做记录（2026-08-11）

- `with_skills` 翻转：registry 非空 → `read_spec()` 进 specs + `index_spec()` 进 timed 区
  （执行体运行时读表内 registry，不是捕获副本）；**空 registry 逐字节零变化**——第一版漏了
  这个门，被独测 `skill_switch_indep` 当场抓红后补上（独立测试制度的直接回报）。
- CLI 与 server 摘掉手动 `skill_index_chunk` 注入（否则经 135 会双份索引）；activate/deactivate
  不再装配（代码留到 141 删）。既有测试改写清单见本次会话记录：activation e2e 改为直接
  `Session::activate_skill`（兼容态），server 四个 http_capabilities_skills 测试改到新格式。
- **顺带修出一个真 bug**：`session_start` 跑在 `seed_after_recover` 之前 → `prefix_init`
  永不落盘 → 重启静默丢索引（详见 135 实做记录）。
- **真机（DeepSeek，providers.toml 真 key）**：第 1 轮模型仅凭索引描述自主调
  `srv:skill/read`，口令「玄武霜降」回答正确；第 9 轮追问时自主重读；十轮
  cached/prompt：98.9/97.9/99.4/99.1/98.1/99.5/97.9/99.1/98.8+97.8/99.3%——**全部 ≥ 0.9
  且含两次 read 轮**；全程零 drift 告警。068 的口令实验在新路上的对照组行为兑现。
