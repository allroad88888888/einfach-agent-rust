#!/usr/bin/env bash
# 架构红线自动检查 —— 只查能被 grep 判定的部分，规则与理由见 docs/INVARIANTS.md
#
#   check-invariants.sh              从 stdin 读 hook JSON，查单个文件，输出 hook JSON
#   check-invariants.sh --all        查全仓，给人看的输出，有违规则退出码 1（CI 用）
#   check-invariants.sh <file>...    查指定文件，给人看的输出
#
# 需要判断的红线（5 大值 Arc、6 epoch）不在这里，走 skill agent-state-design。

set -uo pipefail

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
MAX_HARD=500   # 顶破即违规
MAX_SOFT=300   # 顶破需要「拆了反而更难读」的理由

VIOLATIONS=()
WARNINGS=()

v() { VIOLATIONS+=("$1"); }
w() { WARNINGS+=("$1"); }

# grep -n，但丢掉整行注释 —— 否则「禁止使用 AtomId」这类说明性注释会被当成违规。
# 第三参是注释前缀：Rust 用 //（含文档注释 /// 和 //!），TOML 用 #。
# 已知缺口：块注释 /* */ 不处理，本仓不用块注释。
# 首参可选 -i：忽略大小写。
scan() {
  local flags=""
  if [[ "$1" == "-i" ]]; then flags="-i"; shift; fi
  local pattern="$1" file="$2" comment="${3-//}"
  grep -nE $flags "$pattern" "$file" | grep -vE "^[0-9]+:[[:space:]]*${comment}" || true
}

# 红线 9：文件行数
check_line_count() {
  local f="$1" rel="$2"
  case "$rel" in
    tests/*|*/tests/*|benches/*|*/benches/*|*generated*|*.gen.rs) return 0 ;;
  esac
  local n
  n=$(wc -l < "$f" | tr -d ' ')
  if (( n > MAX_HARD )); then
    v "$rel:1  [红线9] ${n} 行，超过 ${MAX_HARD} 行硬上限。按职责拆分是本次改动的一部分。"
  elif (( n > MAX_SOFT )); then
    w "$rel:1  [红线9] ${n} 行，超过 ${MAX_SOFT}。只有强内聚的单一算法/状态机/引擎核心才允许到 ${MAX_HARD}，说不出「拆了反而更难读」的理由就得拆。"
  fi
}

# 红线 7：agent-core / agent-store 不得做 IO
check_no_io() {
  local f="$1" rel="$2"
  case "$rel" in
    crates/agent-core/*|crates/agent-store/*) ;;
    *) return 0 ;;
  esac
  # 集成测试豁免：红线 7 管的是实现（loop 要能无网络跑单测），不是测试本身——
  # 元测试用 std::process 跑本脚本正是我们想要的。
  case "$rel" in
    */tests/*) return 0 ;;
  esac

  if [[ "$rel" == */Cargo.toml ]]; then
    local hit
    hit=$(scan '^[[:space:]]*(reqwest|hyper|axum|tokio|async-std|ureq|surf)\b' "$f" '#')
    [[ -n "$hit" ]] && v "$rel  [红线7] 禁止的 IO 依赖：
${hit}
    agent-core/agent-store 必须能在无网络下跑完整 loop 的单元测试。IO 放 agent-providers / agent-server。"
    return 0
  fi

  local hit
  # 同时抓 use 语句和全限定调用——`std::fs::read(...)` 不写 use 也能用，
  # 只查 use 的话红线是摆设（001 独测时发现的缺口）。
  hit=$(scan 'use +(std::(fs|net|process)|tokio::|reqwest::)|\bstd::(fs|net|process)::' "$f")
  [[ -n "$hit" ]] && v "$rel  [红线7] 禁止的 IO 引用：
${hit}"
}

# 红线 12：core 里不许有任何模型相关的判断
# 厂商名和能力位分支一起查 —— `if caps.xxx()` 只是 `match provider` 换了层皮。
check_no_model_branch() {
  local f="$1" rel="$2"
  case "$rel" in
    crates/agent-core/*|crates/agent-store/*) ;;
    *) return 0 ;;
  esac

  if [[ "$rel" == */Cargo.toml ]]; then
    local hit
    hit=$(scan '^[[:space:]]*agent-providers\b' "$f" '#')
    [[ -n "$hit" ]] && v "$rel  [红线12] core 依赖了 adapter 层：
${hit}
    依赖方向必须是 agent-providers → agent-core。反过来说明能力位漏进 core 了。"
    return 0
  fi

  [[ "$rel" == *.rs ]] || return 0

  local hit
  hit=$(scan -i '\b(deepseek|kimi|moonshot|zhipu|glm|openai|anthropic|gemini|qwen)\b' "$f")
  [[ -n "$hit" ]] && v "$rel  [红线12] core 里出现厂商名：
${hit}
    模型相关的判断全部归 agent-providers。core 说意图，adapter 决定怎么翻译。"

  hit=$(scan '\b(Capabilities|caps\.)' "$f")
  [[ -n "$hit" ]] && v "$rel  [红线12] core 里出现能力位分支：
${hit}
    能力位分支是 match provider 换了层皮：N 位就是 2^N 种组合，多数永远没跑过。
    改成事后报调整——core 直接说意图，adapter 做不到就在响应里带 Adjustment。
    见 docs/ADAPTER.md。"
}

# 红线 2：业务代码禁止直接调 store.set()
check_no_raw_set() {
  local f="$1" rel="$2"
  [[ "$rel" == *.rs ]] || return 0
  case "$rel" in
    crates/agent-store/src/*|crates/agent-core/src/command/*|*/tests/*|benches/*) return 0 ;;
  esac
  local hit
  hit=$(scan '\bstore\.set\(' "$f")
  [[ -n "$hit" ]] && v "$rel  [红线2] 直接调用 store.set()：
${hit}
    primitive 写入必须走 agent-core 的 command 层，否则这次写入不进 undo log，undo 后状态自相矛盾。"
}

# 红线 4：落盘用 AtomKey，不用 AtomId
check_atomid_not_serialized() {
  local f="$1" rel="$2"
  [[ "$rel" == *.rs ]] || return 0
  # 集成测试豁免（010 合并时裁决）：测试天生同时握接缝两侧——建 atom 要 AtomId、
  # 验落盘要 Serialize，同文件不可避免。红线 4 管的是生产序列化路径（src/）。
  # 与红线 2、9 的 tests 豁免同源。
  case "$rel" in
    */tests/*) return 0 ;;
  esac
  [[ -n "$(scan 'derive\([^)]*Serialize' "$f")" ]] || return 0
  local hit
  hit=$(scan '\bAtomId\b' "$f")
  [[ -n "$hit" ]] && v "$rel  [红线4] 同一文件里既有 Serialize 派生又出现 AtomId：
${hit}
    AtomId 是自增 u64，依赖创建顺序。落盘必须用 AtomKey，否则往构图函数中间插一行 create_atom 就会让所有旧快照静默错位。"
}

# 红线 1：derived 的 read fn 必须是纯函数（粗筛）
# 路径由 issue 026 扩到 graph/：INVARIANTS.md 写的是 `agent-core/src/atoms/`，而 026
# 的裁决把构图函数（derived 的 read fn 真正的住处）放在 `agent-core/src/graph/`
# ——issue 原文允许「构图函数放 graph/ 还是 atoms/ 自定，红线 1 的检查路径跟着改」。
# atoms/ 保留在名单里：M3 若长出那个目录，不必再改一次脚本。
check_derived_purity() {
  local f="$1" rel="$2"
  case "$rel" in
    crates/agent-core/src/atoms/*|crates/agent-core/src/graph/*) ;;
    *) return 0 ;;
  esac
  local hit
  hit=$(scan '(Instant::now|SystemTime::now|Utc::now|rand::|thread_rng|OsRng)' "$f")
  [[ -n "$hit" ]] && v "$rel  [红线1] derived 里出现时钟/随机源：
${hit}
    重放（undo/redo/崩溃恢复）要能得出同样的结果。需要当前时间就做成 primitive atom，由 command 层写入时取值。"
}

# 红线 3：primitive 的值必须可序列化（粗筛）
check_no_opaque_value() {
  local f="$1" rel="$2"
  case "$rel" in
    crates/agent-core/src/value*.rs|crates/agent-core/src/value/*.rs) ;;
    *) return 0 ;;
  esac
  local hit
  hit=$(scan 'dyn +Any' "$f")
  [[ -n "$hit" ]] && v "$rel  [红线3] AgentValue 里出现 dyn Any：
${hit}
    不可序列化的活对象放 store 外的 runtime registry，atom 里只放可序列化句柄。给了这种变体就一定有人塞，然后快照有洞。"
}

# 红线 8：bind 默认 loopback
check_bind_default() {
  local f="$1" rel="$2"
  case "$rel" in
    crates/agent-server/*|crates/agent-server-bin/*) ;;
    *) return 0 ;;
  esac
  # tests 豁免：验证「0.0.0.0 必须显式」的测试自己就得写这个字面量。
  # 与红线 2/4/7/9 的 tests 豁免同源；红线 8 管的是 src 里的默认值。
  case "$rel" in
    */tests/*) return 0 ;;
  esac
  local hit
  hit=$(scan '0\.0\.0\.0' "$f")
  [[ -n "$hit" ]] && w "$rel  [红线8] 出现硬编码 0.0.0.0：
${hit}
    默认必须绑 127.0.0.1，监听 0.0.0.0 只能由 AGENT_BIND 显式打开。当前没有任何鉴权。"
}

# 红线 11：会进 prompt 的东西，序列化必须逐字节确定
check_deterministic_serde() {
  local f="$1" rel="$2"
  [[ "$rel" == *.rs ]] || return 0
  [[ -n "$(scan 'derive\([^)]*Serialize' "$f")" ]] || return 0
  local hit
  hit=$(scan '(HashMap|HashSet)<' "$f")
  [[ -n "$hit" ]] && v "$rel  [红线11] 可序列化类型里出现无序容器：
${hit}
    前缀缓存靠逐字节相等，HashMap 迭代顺序在 Rust 里是随机化的。顶层 tools 又在 prompt 最前面，顺序一漂每轮都全价（DeepSeek 上 120x）。改用 BTreeMap / BTreeSet / Vec。"
}

check_file() {
  local f="$1"
  [[ -f "$f" ]] || return 0
  local rel="${f#"$REPO"/}"
  case "$rel" in
    target/*|*/target/*|node_modules/*|*/node_modules/*|.git/*) return 0 ;;
  esac
  case "$rel" in
    *.rs|Cargo.toml|*/Cargo.toml) ;;
    *) return 0 ;;
  esac

  check_line_count           "$f" "$rel"
  check_no_io                "$f" "$rel"
  check_no_model_branch      "$f" "$rel"
  check_no_raw_set           "$f" "$rel"
  check_atomid_not_serialized "$f" "$rel"
  check_derived_purity       "$f" "$rel"
  check_no_opaque_value      "$f" "$rel"
  check_bind_default         "$f" "$rel"
  check_deterministic_serde  "$f" "$rel"
}

emit_human() {
  local rc=0
  if ((${#VIOLATIONS[@]})); then
    printf '\n红线违规：\n\n'
    printf '  %s\n\n' "${VIOLATIONS[@]}"
    rc=1
  fi
  if ((${#WARNINGS[@]})); then
    printf '\n提示：\n\n'
    printf '  %s\n\n' "${WARNINGS[@]}"
  fi
  ((rc == 0 && ${#WARNINGS[@]} == 0)) && printf '红线检查通过\n'
  printf '规则与理由：docs/INVARIANTS.md\n'
  return $rc
}

emit_hook() {
  local body=""
  ((${#VIOLATIONS[@]})) && body+="架构红线违规（docs/INVARIANTS.md）："$'\n\n'"$(printf '%s\n\n' "${VIOLATIONS[@]}")"
  ((${#WARNINGS[@]}))   && body+=$'\n'"提示："$'\n\n'"$(printf '%s\n\n' "${WARNINGS[@]}")"
  [[ -z "$body" ]] && exit 0

  if ((${#VIOLATIONS[@]})); then
    jq -nc --arg r "$body" '{decision:"block", reason:$r}'
  else
    jq -nc --arg r "$body" '{hookSpecificOutput:{hookEventName:"PostToolUse", additionalContext:$r}}'
  fi
}

case "${1-}" in
  --all)
    # tracked + 未被 ignore 的 untracked，本地跑和 CI 跑口径一致
    while IFS= read -r f; do check_file "$f"; done < <(
      cd "$REPO" && {
        git ls-files '*.rs' '*Cargo.toml' 2>/dev/null
        git ls-files --others --exclude-standard '*.rs' '*Cargo.toml' 2>/dev/null
      } | sort -u | sed "s|^|$REPO/|"
    )
    # 197：顺带看一眼构建缓存有没有胀。**独立脚本**——磁盘胀不是架构红线，
    # 只是搭这趟已经在跑的车（收工 / CI），不进 hook 那条高频路径。
    bash "$REPO/scripts/check-build-cache.sh" --quiet
    emit_human
    ;;
  "")
    # hook 模式：从 stdin 读 JSON
    input=$(cat)
    path=$(printf '%s' "$input" | jq -r '.tool_input.file_path // .tool_response.filePath // empty' 2>/dev/null)
    [[ -z "$path" ]] && exit 0
    check_file "$path"
    emit_hook
    ;;
  *)
    for f in "$@"; do check_file "$f"; done
    emit_human
    ;;
esac
