# 182 `einfach-store` 首发 crates.io

**里程碑** L · **依赖** [181](181-store-publish-prep.md) · **谁做** **用户**（按下去的那一下） · **估时** 10min · **状态** 流水线已就绪，等你填 secret + 打 tag

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
