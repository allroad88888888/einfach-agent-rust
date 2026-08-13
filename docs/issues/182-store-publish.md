# 182 `einfach-store` 首发 crates.io

**里程碑** L · **依赖** [181](181-store-publish-prep.md) · **谁做** 用户（填 secret + 推 tag）+ 我（其余） · **估时** 10min · **状态** ✅ **已发布**（2026-08-13，`einfach-store 0.1.0`）

## 目标

发出去。

## 为什么最后一下必须用户做

crates.io 发布需要账号与 API token，且**发布不可逆**——版本发出去只能 yank 不能删，
名字一旦占用不能转让给别人。这类动作要本人按下去。

**但「本人按下去」不等于「在本机 `cargo login`」。** 2026-08-13 改走 CI
（`.github/workflows/release.yml`）：token 只作为 GitHub secret 存在，
注入一次运行的进程环境，跑完就没了，不会在开发机上长期躺着。
按下去的那一下变成「打一个 tag」，一样是本人的显式动作，而且留痕。

顺带解决一个本机专有的坑：这台机器的 `.cargo/config.toml` 把 crates-io 换成了
rsproxy 镜像，`cargo publish` 会直接报 `crates-io is replaced with non-remote-registry
source` 退出。CI 环境没有这层替换。

## 做什么

**一、一次性配置**（GitHub 界面，token 不经过任何对话）

1. 注册 crates.io（GitHub 账号登录），Account Settings → API Tokens 生成一个
   token，scope 勾 `publish-new` + `publish-update`。
2. 仓库 Settings → Secrets and variables → Actions → New repository secret，
   名字 **`CARGO_REGISTRY_TOKEN`**，值粘贴那个 token。

**二、先干跑一遍流水线**（不上传，验这条路本身通不通）

Actions → Release (crates.io) → Run workflow，`dry_run` 保持勾选。

**三、真发**

```bash
git tag einfach-store-v0.1.0
git push origin einfach-store-v0.1.0
```

tag 名的版本号与 `Cargo.toml` 不一致会在发布前被拦下——tag 写错版本是这类流水线
最经典的翻车，而发出去的版本删不掉。

**四、发布后核对**（流水线最后一步会把这三条打出来）

crates.io 页面 README 渲染正常、license 显示 `MIT OR Apache-2.0`、
docs.rs 构建成功（**docs.rs 构建失败是首发最常见的翻车点**，它跟本地
`cargo doc` 的环境不一样）。

## 本机预检（2026-08-13 已跑）

- `cargo publish -p einfach-store --dry-run --registry crates-io`：
  `Packaged 66 files, 338.6KiB (99.7KiB compressed)`，校验编译通过
- 打包清单除源码与测试外只有 `Cargo.toml` / `Cargo.lock` / `README.md` /
  `.cargo_vcs_info.json`，无夹带
- `cargo doc -p einfach-store --no-deps`：**0 error**，6 条 warning 全是
  文档链接的美观问题（3 条链到私有项、3 条冗余目标），docs.rs 不会因此失败
- crates.io API 确认 `einfach-store` 名字未被占用

## 验收

- `cargo add einfach-store` 在一个空项目里可用
- docs.rs 上文档构建成功且能打开
- crates.io 页面的 license 显示正确

## 之后

- 主仓 README 加 crates.io badge
- 这条完成后 L1 波收官，可以进 L2 内容波

## 注意

**版本号定 `0.1.0` 还是 `0.0.1`？** 建议 `0.1.0`——`0.0.x` 传达「随时会崩」，
而这个 crate 的核心逻辑是 fork 自已在生产用的上游引擎、且本仓有完整测试覆盖。
但要在 README 里诚实写明 API 尚未稳定。

## 实做记录（2026-08-13）

**已发布**：`einfach-store 0.1.0`，crates.io API 核对——`license: MIT OR Apache-2.0`、
`yanked: false`、102,060 bytes、keywords 与 categories 都在。tag `einfach-store-v0.1.0`
指向 `908037b`。

### 真发布第一次是红的，值得记

第一次推 tag（run `31705598151`）跑到最后一步失败：

```
the remote server responded with an error (status 400 Bad Request):
A verified email address is required to publish crates to crates.io.
```

**token 是好的**——请求走到了上传拿到 400 而不是 401/403，卡的是账号没验证邮箱。
验证后 `gh run rerun --failed` 重跑同一个 run 即通过，**不用删 tag、不用换版本号**：
失败发生在上传，crates.io 上什么都没落下，`0.1.0` 还是干净的。

这一条正是「先干跑」那道闸的价值边界：干跑验得了打包和编译，验不了账号状态——
它压根不发请求。所以真发布第一次红是正常的，重要的是**它红在一个没有副作用的地方**。

### 三条验收

- **`cargo add` 在空项目里可用** ✅ 写了个真用得上的冒烟测试（不是 `cargo build` 就算过）：
  从 registry 装上 → `record_set` 写 42 → `apply_prev` 回滚 → 断言值回到 1。
  也就是说这个包的卖点本身，在**下游视角**下验过一次。
- **crates.io license 显示正确** ✅ `MIT OR Apache-2.0`
- **docs.rs 构建成功** ✅ 发布后约 8 分钟。前 3 分钟一直 404，中途查了
  `https://docs.rs/releases/queue` 确认命中 `einfach-store`——**在队列里，不是失败**，
  这一步值得单独查一次：404 分不出「排队中」和「构建挂了」。
  最终 `https://docs.rs/einfach-store/0.1.0/einfach_store/` 返回 200，
  页面 24KB，`AtomFamily` / `AtomId` / `Store` / `History` / `Snapshot` / `apply_prev` /
  `record_set` / `SessionStore` 八个公开项全部列出——**不是能打开就算过，要确认不是空壳**。

### 顺带

README（中英）加了 badge，并且明写「**目前只有一个 crate 上了 crates.io**」——
`einfach-agent` 整个运行时并没发布。badge 特意带 `?label=einfach-store`：
不加 label 的话，一个写着 `crates.io v0.1.0` 的徽章会被读成「这个项目在 crates.io 上」。
