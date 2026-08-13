# 最后一公里：只有你能做的四件事

L 波（对外推广，[issue 165–198](../issues/README.md#l--对外推广)）我这边能做的都做完了。
剩下的四件都要你本人——**不是没做，是性质上要你按下去**：发布是外发动作、账号绑定你的身份、
联系真人要你出面。这份文件把散在三条 issue 里的东西按**执行顺序**合成一张单子。

每一步都标了「**为什么是这个顺序**」——顺序是这一波里少数几个不可逆的决定之一
（Show HN 同一个项目发第二次，效果差很多）。

---

## 第 0 步 · push（一条命令）

```bash
git push
```

**这一步先做，因为后面每一步都依赖它。** 当前本地领先 `origin/main` 若干个提交，
其中包含：

- `/roles.html` 第二个 demo（**现在线上是 404**，推上去才活）
- 主 demo 的英文化 + 新的英文系统提示词（不推的话，英文访客问英文、agent 答中文）
- `docs/assets/roles-demo.gif`，README 里已经引用了它——**不推就是一张裂图**
- 五篇文章的中英两版、CI/Pages 配置

推上去之后 Pages 会自动部署（`pages.yml` 的 paths 命中 `crates/agent-wasm/**`）。

**然后点这三个 URL 确认**（发帖前还要再点一次，见第 3 步）：

```
https://allroad88888888.github.io/einfach-agent-rust/                       → 200
https://allroad88888888.github.io/einfach-agent-rust/roles.html             → 200（现在是 404）
https://allroad88888888.github.io/einfach-agent-rust/pkg/agent_wasm_bg.wasm → 200
```

第三条别省：`index.html` 是静态文件，**它 200 不代表 demo 能用**，
而一个打得开但一按就报错的 demo 比 404 更伤。

---

## 第 1 步 · 发第一篇文章（[183](../issues/183-post-providers.md)）

**发[《DeepSeek、Kimi、GLM 到底差在哪》](183-three-providers.en.md)**，不是发首发帖。

为什么它打头：**它的价值独立于这个项目存在**。读者不需要关心你写了什么，
只需要关心他自己在用的那三家模型。这类内容有长尾搜索流量，也最可能被转。

发之前：数字的地基（`tool_choice` 与思考模式互斥那两条）**已于 2026-08-13 重跑复现**，
七格全表 + 两条错误原文逐字一致。要再验一次的话：

```bash
cd probes/api && PROBE_TOOL_CHOICE_ONLY=1 cargo run --bin wire_shape
```

（三家各三个请求，比全套十几个便宜。）

---

## 第 2 步 · 看反响，再决定要不要发第二篇（[185](../issues/185-post-invariants.md)）

[185《不会报错的那几类 bug》](185-bugs-that-dont-fail.en.md) 是十二条红线的全景。
**它必须排在 [186](186-capability-flags.en.md) 之前**——186 开头回链 185，
反过来发就指向一篇还没发的文章。

186 再隔**至少一周**。同一周发两篇讲同一条规则会显得素材单薄；
隔开了才读起来像「有人追问了那条，所以展开讲」。

184 和 187 不占顺序位，插在哪都行。

---

## 第 3 步 · Show HN + r/rust（[191](../issues/191-launch-post.md)）

**两套文案在 [`191-launch-posts.md`](191-launch-posts.md)，不是同一篇复制。**
评论区常见问题的预案（跟 rig 的区别 / 为什么 Rust / 为什么只有中国模型 /
生产用了吗 / 不 Send/Sync 是不是限制 / 这不就是 event sourcing 吗）也在那份文件里。

**发之前六条前置，现在的状态：**

| | |
|---|---|
| demo 链接当天验过 | ⏳ **push 之后当天再点一次**（这条的性质就是「当天有效」） |
| README 第一屏有 demo + GIF | ✅ |
| 陌生人能走通的试用路径 | ✅ demo 自带 key，公网 CORS 已验 |
| LICENSE | ✅ 双许可 |
| **至少一篇文章已发并有反响** | ⏳ **第 1 步做完才满足——目前唯一硬挡着的一条** |
| **你有一整天能守评论区** | ⏳ 只有你知道 |

时机：美西时间工作日早上，避开大新闻日。

**「生产用了吗」这条的答案是「没有」**，而且要直说——别把 dogfood 说成生产验证。
含糊一次，前面所有诚实标注积累的信任就都抵掉了。

---

## 第 4 步 · crates.io（[182](../issues/182-store-publish.md)）

**可以晚于首发，但别太晚**——首发之后有人想试，`cargo add` 拿不到会流失。

```bash
# 1. 注册 crates.io（GitHub 登录）后
cargo login

# 2. 先干跑，确认无误
cargo publish -p einfach-store --dry-run

# 3. 真发
cargo publish -p einfach-store
```

版本号建议 **`0.1.0`** 而不是 `0.0.1`：`0.0.x` 传达「随时会崩」，
而这个 crate 的核心 fork 自已在生产使用的上游引擎、且本仓测试覆盖完整。
但 README 里要诚实写明 API 尚未稳定（已经写了）。

**发布不可逆**：版本只能 yank 不能删，名字一旦占用不能转让。

发完核对四件：README 渲染正常、license 显示 `MIT OR Apache-2.0`、
`cargo add einfach-store` 在空项目里可用、**docs.rs 构建成功**——
最后这条是首发最常见的翻车点，docs.rs 的环境跟本地 `cargo doc` 不一样。

---

## 第 5 步 · 找 3–5 个真实嵌入用户（[194](../issues/194-early-adopters.md)）

首发之后开始。问法、记录格式、判据都在 [194](../issues/194-early-adopters.md) 里，
这里只重复三条最容易做错的：

- **别在对话里承诺排期。** 记下来就够了。当场答应会把「一个人提的」变成事实上的需求。
- **留原话，别转述。** 转述会不自觉地把对方的话翻译成你本来就想做的事。
- **一个人提的不是需求，三个人提的才是。** 每条障碍标上谁提的，同一条被三个不同来源
  提到才进路线图。

**并且显式记「没人提的东西」**——多租户、多副本 `RedisRegistry`、MCP 的 OAuth，
如果五个人里没人问，那个「没人问」本身要写下来，否则下次又会有人凭直觉觉得该做了。

---

## 我这边留下的东西在哪

| | |
|---|---|
| 五篇文章，中英各一版 | [`docs/posts/`](README.md) |
| 两套首发文案 + 评论区预案 | [`191-launch-posts.md`](191-launch-posts.md) |
| 两张 GIF | `docs/assets/undo-demo.gif`、`docs/assets/roles-demo.gif` |
| 三份英译文档 | `docs/{INVARIANTS,ARCHITECTURE,STATE-MODEL}.en.md`，顶部钉了译自哪个 commit |
| 逐条实做记录 | [`docs/issues/165–198`](../issues/README.md) |
