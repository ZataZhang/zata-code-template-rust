#!/usr/bin/env bash
# 提交信息检查（pre-commit 的 commit-msg 阶段调用）。规则见 docs/ai-standards/tooling.md：
#   - Conventional Commits：type(scope): subject
#   - 标题行不超过 72 个字符
#   - 标题用英文（不含中日韩字符）
# 合并提交、revert、fixup!/squash! 提交不检查。
#
# 用法：scripts/check-commit-msg.sh <提交信息文件>
#       scripts/check-commit-msg.sh --self-test     # 跑内置的正反例，确认检查本身有效
set -euo pipefail

readonly TYPES='feat|fix|refactor|docs|test|chore|perf|build|ci|style|revert'
readonly PATTERN="^(${TYPES})(\\([a-z0-9._/-]+\\))?!?: .+"
readonly MAX_SUBJECT_CHARS=72

# 检查一行标题；通过返回 0，否则向 stderr 说明原因并返回 1。
check_subject() {
  local subject="$1"
  case "$subject" in
    "Merge "*|"Revert \""*|"fixup! "*|"squash! "*) return 0 ;;
  esac
  if ! [[ "$subject" =~ $PATTERN ]]; then
    echo "提交标题不符合 Conventional Commits：type(scope): subject" >&2
    echo "  type 可选：${TYPES//|/, }" >&2
    echo "  实际：$subject" >&2
    return 1
  fi
  if [ "${#subject}" -gt "$MAX_SUBJECT_CHARS" ]; then
    echo "提交标题超过 ${MAX_SUBJECT_CHARS} 个字符（当前 ${#subject}）：$subject" >&2
    return 1
  fi
  # 标题必须是英文：出现任何非 ASCII 字符就拒绝。
  if LC_ALL=C grep -q '[^ -~]' <<<"$subject"; then
    echo "提交标题必须使用英文（含非 ASCII 字符）：$subject" >&2
    return 1
  fi
  return 0
}

self_test() {
  local failures=0
  expect() { # expect pass|fail "标题"
    local want="$1" subject="$2" got=pass
    check_subject "$subject" 2>/dev/null || got=fail
    if [ "$got" != "$want" ]; then
      echo "自检失败：期望 $want，实际 $got：$subject" >&2
      failures=$((failures + 1))
    fi
  }
  expect pass "feat(auth): add login rate limiting"
  expect pass "fix: handle empty password"
  expect pass "refactor(api)!: rename error codes"
  expect pass "docs: update README"
  expect pass "Merge branch 'main' into feature"
  expect pass "fixup! feat: add x"
  expect fail "added some stuff"
  expect fail "Feat: capitalised type"
  expect fail "feat:no space after colon"
  expect fail "feat: "
  expect fail "wip(auth): something"
  expect fail "feat(auth): 添加登录限流"
  expect fail "feat: $(printf 'x%.0s' $(seq 1 80))"
  if [ "$failures" -ne 0 ]; then
    echo "commit-msg 检查自检未通过（$failures 项）" >&2
    return 1
  fi
  echo "commit-msg 检查自检通过"
}

main() {
  if [ "${1:-}" = "--self-test" ]; then
    self_test
    return
  fi
  local message_file="${1:?用法：$0 <提交信息文件> | --self-test}"
  local subject
  subject="$(grep -v '^#' "$message_file" | sed -n '1p')"
  check_subject "$subject"
}

main "$@"
