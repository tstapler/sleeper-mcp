#!/usr/bin/env bash
# Local entry point for Bazel: bazelisk with a bounded, known output location and a TMPDIR outside $HOME.
#   scripts/bazel.sh test //...
#   scripts/bazel.sh prune        # bazel clean for this workspace (the output base is rebuilt on demand)
# The disk cache is NOT set here: ~/.bazelrc owns the shared, size-capped cache (do not override it in the repo).
# Environment (all optional):
#   SLEEPER_BAZEL_OUTPUT_ROOT  output user root (default ~/.cache/sleeper-bazel). The output base of each checkout or
#                              worktree is a hashed subdirectory of it; `prune` empties the current one, and
#                              `prune --all` removes every output base under the root.
#   SLEEPER_BAZEL_PLAIN=1      pass arguments through unchanged (CI: setup-bazel owns the caches and output base)
# Why TMPDIR: crate_universe splices Cargo workspaces under $TMPDIR and refuses to run when a ~/.cargo/config.toml is
# found in a parent directory, which is the case for any TMPDIR below $HOME.
set -euo pipefail
root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
cd "$root"
case "${TMPDIR:-}" in "" | "$HOME"/*) export TMPDIR=/tmp ;; esac
out=${SLEEPER_BAZEL_OUTPUT_ROOT:-$HOME/.cache/sleeper-bazel}
bz=${SLEEPER_BAZELISK:-$(command -v bazelisk || command -v bazel || true)}
[ -n "$bz" ] || { echo "scripts/bazel.sh: neither bazelisk nor bazel is on PATH (brew install bazelisk)" >&2; exit 127; }
if [ "${SLEEPER_BAZEL_PLAIN:-0}" = 1 ]; then exec "$bz" "$@"; fi
if [ $# -eq 0 ]; then exec "$bz" --output_user_root="$out"; fi
cmd=$1; shift
case $cmd in
  prune)
    if [ "${1:-}" = --all ]; then
      "$bz" --output_user_root="$out" shutdown || true
      rm -rf -- "${out:?}"
    else
      "$bz" --output_user_root="$out" clean --expunge
    fi ;;
  *) exec "$bz" --output_user_root="$out" "$cmd" "$@" ;;
esac
