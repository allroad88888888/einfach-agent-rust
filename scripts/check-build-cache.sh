#!/usr/bin/env bash
# 构建缓存体积阈值检查（issue 197）。**跟红线检查是两件事**——磁盘胀不是架构错误，
# 所以不塞进 `check-invariants.sh`（那个文件管的是「能被 grep 判定的架构红线」）。
#
# 这是 197 那条「一个会被定期检查的数字」的兑现。没有它，
# `scripts/clean-build-cache.sh` 迟早变成没人跑的死代码——58GB 那次（2026-08-05）
# 的教训正是这个形状：清了、改了测试组织、写进文档，然后八天后从另一个口子长回
# 31G，中间**没有任何东西提醒过**。
#
# **不进 PostToolUse hook**：`du` 在 target 胀起来的时候恰好最慢，装进每次 Edit
# 就成了「最需要它的时候最碍事」。挂在 `check-invariants.sh --all`（收工 / CI）上，
# 那条路本来就不频繁。
#
# 退出码恒为 0：**这是提示不是门禁**。让 CI 因为开发机磁盘胀而变红是错的信号
# ——CI 的 target 每次都是新的，它永远不会触发这条。
#
# 用法：
#   scripts/check-build-cache.sh          # 超阈值则提示
#   scripts/check-build-cache.sh --quiet  # 只在超阈值时才输出（给 --all 串联用）
set -uo pipefail

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

# 阈值 20G：清理后是 9G，日常开发涨几 G 正常（实测一个开发会话 +4G），
# 到 20G 说明该清了。定得太低会天天响，响多了就没人看——那跟没有一样。
LIMIT_GB=20

quiet=0
[[ "${1-}" == "--quiet" ]] && quiet=1

total=0
for t in "$REPO/target" "$REPO/crates/agent-wasm/target" \
         "$REPO/probes/api/target" "$REPO/apps/desktop/src-tauri/target"; do
  [[ -d "$t" ]] || continue
  kb=$(du -sk "$t" 2>/dev/null | cut -f1) || continue
  total=$((total + kb))
done

gb=$((total / 1024 / 1024))

if ((gb >= LIMIT_GB)); then
  printf '\n构建缓存 %dG（阈值 %dG）\n\n' "$gb" "$LIMIT_GB"
  printf '  跑 scripts/clean-build-cache.sh —— 只清可再生的中间产物\n'
  printf '  （incremental / *.rcgu.o / 非原生目标目录），不动 .rlib，\n'
  printf '  所以下次构建不会从零开始。\n\n'
  printf '  理由与实测：docs/issues/197-incremental-cache-bloat.md\n\n'
elif ((quiet == 0)); then
  printf '构建缓存 %dG（阈值 %dG）\n' "$gb" "$LIMIT_GB"
fi

exit 0
