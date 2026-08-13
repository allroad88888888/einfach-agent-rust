# 对外文章

L 波（对外推广）产出的文章初稿。**每一篇的素材都来自仓库里真实发生过的事**——
实测数据、被自己删掉的子系统、构建事故、踩过的坑，没有一篇是为了发文现编的。

> **这些是初稿，一篇都没发。** 「发」是外发动作，由仓库主人决定发在哪、
> 用什么身份、什么时候发。
>
> **要发的话先看 [LAST-MILE.md](LAST-MILE.md)** —— 把发文、首发帖、crates.io、
> 找用户四件事按顺序排好了，顺序本身是这一波里少数不可逆的决定之一。

## 为什么在版本控制里

它们最初写在会话的临时目录里。那是错的：本仓的 `scratchpad` 历来只放**用完即弃**的
东西（探针、临时 `CARGO_TARGET_DIR`、dogfood 脚本），而这几篇是交付物——
**六千多词的交付物躺在一个会话结束就消失的目录里，等于没做。**

放进 `docs/` 还有第二个理由：文章里的每个数字都指向仓库里的某个文件，
**它们和被引用的东西必须一起变**。分开放，第一次改文档就开始漂。

## 清单

| 文件 | 讲什么 | 出处 |
|---|---|---|
| [183 三家实测差异](183-three-providers.en.md) | 缓存语义 / usage 分帧 / `tool_choice` 与思考模式互斥 | [issue 183](../issues/183-post-providers.md)，数据在 [PROVIDERS.md](../../probes/PROVIDERS.md) |
| [184 删掉自己的子系统](184-deleted-my-own-subsystem.en.md) | 为改进它而做的测量，最后杀死了它（净减 1945 行） | [issue 184](../issues/184-post-decision-27.md)，决策 27 |
| [185 不会报错的那几类 bug](185-bugs-that-dont-fail.en.md) | 十二条红线里六条违反后不报错 | [issue 185](../issues/185-post-invariants.md)，[INVARIANTS](../INVARIANTS.en.md) |
| [186 能力位是 `match provider` 换层皮](186-capability-flags.en.md) | 185 里最后一条的长版 | [issue 186](../issues/186-post-adapter-seam.md)，[ADAPTER](../ADAPTER.md) |
| [187 两天 58GB](187-target-bloat.en.md) | 构建事故，以及**修完之后它从别处又长了回来** | [issue 187](../issues/187-post-target-bloat.md)，[issue 197](../issues/197-incremental-cache-bloat.md) |
| [191 首发帖文案](191-launch-posts.md) | Show HN 与 r/rust 两套，**不是同一篇复制** | [issue 191](../issues/191-launch-post.md) |

**每篇都有中英两版**（`*.en.md` / `*.zh-CN.md`）。英文版先写
（[决策 165 L1](../issues/165-launch-positioning-decision.md)：英文社区优先），
中文版按中文的节奏重写，**不是逐句直译**——但两版的承重数字逐个比对过，
一致（比对是脚本做的，不靠眼看：这几篇的全部可信度就挂在那些数字上）。

首发文案（191）只有中文——它是给仓库主人看的操作材料，不是文章。

## 发布顺序（[issue 186](../issues/186-post-adapter-seam.md) 定的）

```
183  →  185  →（隔一周）→ 186  →  Show HN
```

- **183 打头**：独立价值最高，最不依赖读者关心本项目，也最可能被转
- **185 在 186 之前**：186 的开头回链 185，反过来发就指向一篇还没发的文章
- **Show HN 最后**：带着一篇没人看过的文章去发首发帖，等于把最能证明
  「这人认真」的材料浪费掉

184 与 187 不占顺序位——两篇都能独立站住，插在哪都行。

## 发之前

**数字会过期，而文章里的数字是它全部的可信度。**

183 的地基（`tool_choice` 与思考模式互斥那两条）已于 2026-08-13 重跑复现，
跑法与理由见 [issue 183](../issues/183-post-providers.md) 的待办。其余几篇引的是
仓库自身的数字（行数、体积、文件数），改动大了要重量一遍。
