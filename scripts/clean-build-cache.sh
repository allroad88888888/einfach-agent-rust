#!/usr/bin/env bash
# 清构建缓存（issue 197）。**不是 `cargo clean`**——那会连 `.rlib` 一起删掉，
# 下一次全量重编几分钟起步。这个脚本只砍三样**可再生的中间产物**：
#
# 1. `incremental/` —— 增量编译状态。按「crate × 编译配置」分，本仓配置组合不少
#    （`ts` feature、`--all-targets`、native/wasm32 两个目标），每个组合各留一份
#    历史。代价只有「下一次编译不走增量」。
#
# 2. `deps/**/*.rcgu.o` —— **体积大头，也是文件数大头**。`.rcgu.o` 是每个
#    codegen unit 一个目标文件（dev profile 默认 `codegen-units=256`），
#    **按构建 hash 分开存，而 cargo 从不回收旧 hash 的那些**。
#    2026-08-13 实测：`agent_cli` 一个 crate 就攒了 **40 个构建 hash、42,732 个
#    `.o`**；全仓 `deps/` 里 **631,526 个 `.o` 占 ~11.4G**，而真正的产物
#    （`.rlib` 1.7G + `.rmeta` 906M）只有 2.6G。`.o` 是链接的中间产物，
#    `.rlib` 已经链好了——删掉最坏就是下次重新生成。
#
# 3. **非原生、非 wasm 的目标目录** —— 一次性交叉编译留下的孤儿。
#    `wasm32-unknown-unknown` **不在此列且必须保留**：它是 `build-wasm.sh` 的产物，
#    浏览器宿主（M13/M14 的第三种形态、Pages demo）靠它，不是「另一个平台的顺带产物」。
#
# 文件数比体积更值得盯：rustc 启动要枚举 `deps/`，几十万个条目就是分钟级——
# **构建自己拖慢自己**。2026-08-13 首次执行后 `deps/` 从 63 万文件降到 3,227。
#
# 所以这件事的正确形态是**定期执行的脚本**，不是某次事故后的一次性清理。
# 58GB 那次（2026-08-05）就是按一次性处理的：清了、改了测试组织、写进文档，
# 然后八天后从另一个口子长回 31G。
#
# 用法：
#   scripts/clean-build-cache.sh          # 清 incremental，报清理前后
#   scripts/clean-build-cache.sh --dry    # 只报，不删
#   scripts/clean-build-cache.sh --all    # 连编译产物一起清（等价 cargo clean，慢）
#   scripts/clean-build-cache.sh --files  # 顺带数文件数
#
# **默认不数文件数**：`find | wc -l` 在八十万文件上要跑好几分钟——这个讽刺本身
# 就是问题的一部分（文件多到连「有多少文件」都变成一次慢操作，而那正是当初
# 构建变慢的机制：rustc 启动时要枚举 deps 目录）。想看就加 `--files`。
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

# 四个独立 workspace，各有自己的 target（见 CLAUDE.md §Workspace）。
targets=(
  "$root/target"
  "$root/crates/agent-wasm/target"
  "$root/probes/api/target"
  "$root/apps/desktop/src-tauri/target"
)

dry=0
all=0
count_files=0
for arg in "$@"; do
  case "$arg" in
    --dry) dry=1 ;;
    --all) all=1 ;;
    --files) count_files=1 ;;
    *) echo "未知参数：$arg（可选 --dry / --all / --files）" >&2; exit 2 ;;
  esac
done

# 报一遍各 target 的大小，并把合计（KB）写进全局 REPORT_TOTAL。
REPORT_TOTAL=0
report() {
  local total=0 kb
  echo "$1"
  for t in "${targets[@]}"; do
    [ -d "$t" ] || continue
    kb=$(du -sk "$t" 2>/dev/null | cut -f1)
    total=$((total + kb))
    if [ "$count_files" = 1 ]; then
      printf "  %-42s %7s  %s 文件\n" "${t#"$root"/}" \
        "$(du -sh "$t" 2>/dev/null | cut -f1)" \
        "$(find "$t" -type f 2>/dev/null | wc -l | tr -d ' ')"
    else
      printf "  %-42s %7s\n" "${t#"$root"/}" "$(du -sh "$t" 2>/dev/null | cut -f1)"
    fi
  done
  printf "  %-42s %6sG\n" "合计" "$((total / 1024 / 1024))"
  REPORT_TOTAL=$total
}

report "清理前："
before=$REPORT_TOTAL

if [ "$dry" = 1 ]; then
  echo
  echo "（--dry：什么都没删）"
  exit 0
fi

# 本机原生三元组。除它和 wasm32 之外的目标目录都是一次性交叉编译的孤儿。
native="$(rustc -vV 2>/dev/null | awk '/^host:/{print $2}')"

echo
for t in "${targets[@]}"; do
  [ -d "$t" ] || continue
  rel="${t#"$root"/}"

  if [ "$all" = 1 ]; then
    echo "  清空 $rel"
    rm -rf "${t:?}"/*
    continue
  fi

  # 1) 增量编译状态
  if [ -d "$t/debug/incremental" ]; then
    echo "  删 $rel/debug/incremental"
    rm -rf "$t/debug/incremental"
  fi

  # 2) codegen unit 目标文件（体积与文件数的大头）
  n=$(find "$t" -name "*.rcgu.o" -type f 2>/dev/null | wc -l | tr -d ' ')
  if [ "$n" -gt 0 ]; then
    echo "  删 $rel 下 $n 个 *.rcgu.o"
    find "$t" -name "*.rcgu.o" -type f -delete 2>/dev/null || true
  fi

  # 3) 非原生、非 wasm 的目标目录（wasm32 必须留，见文件头第 3 条）
  for d in "$t"/*/; do
    name="$(basename "$d")"
    case "$name" in
      debug|release|doc|package|tmp|CACHEDIR.TAG) continue ;;
      wasm32-*) continue ;;
      "$native") continue ;;
    esac
    # 只认长得像目标三元组的（含两个以上 `-`），别误删别的东西。
    if [ "$(echo "$name" | tr -cd '-' | wc -c)" -ge 2 ]; then
      echo "  删 $rel/$name（非原生目标，孤儿）"
      rm -rf "$d"
    fi
  done
done

echo
report "清理后："
after=$REPORT_TOTAL

echo
freed_kb=$((before - after))
if [ "$freed_kb" -ge 1048576 ]; then
  # 保一位小数：整数除法会把「释放了 900M」印成「释放 0G」，
  # 一个说谎的报告比没有报告糟。
  printf "释放 %d.%dG\n" $((freed_kb / 1048576)) $(((freed_kb % 1048576) * 10 / 1048576))
else
  printf "释放 %dM\n" $((freed_kb / 1024))
fi
