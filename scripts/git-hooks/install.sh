#!/usr/bin/env bash
# Install wdotool's git hooks. Idempotent. Symlinks so editing the source
# updates the install.
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
hook_src_dir="$repo_root/scripts/git-hooks"

git_dir="$(git -C "$repo_root" rev-parse --git-dir 2>/dev/null || true)"
if [[ -z "$git_dir" ]]; then
  echo "not a git repo: $repo_root" >&2
  exit 1
fi
if [[ "$git_dir" != /* ]]; then
  git_dir="$repo_root/$git_dir"
fi

hooks_dir="$git_dir/hooks"
mkdir -p "$hooks_dir"
ln -sfn "$hook_src_dir/post-commit-jira" "$hooks_dir/post-commit"
echo "  ok   $repo_root"
