#!/usr/bin/env bash
# Fail only when this change leaves lines that rustfmt would rewrite.
#
# The tree carries pre-existing formatting drift, so a repo-wide
# `cargo fmt --check` gate would be red the day it is added. This script scopes
# the check to the line ranges the change actually introduced.
#
# Usage: check-fmt-changed.sh [<base-ref>]
#   base defaults to origin/$GITHUB_BASE_REF on a pull request, else origin/main.
# A push to the base branch compares against itself and passes; the pull request
# carrying the change is where the gate bites.
set -euo pipefail

BASE="${1:-}"
if [ -z "$BASE" ]; then
  if [ -n "${GITHUB_BASE_REF:-}" ]; then
    BASE="origin/$GITHUB_BASE_REF"
  else
    BASE="origin/main"
  fi
fi
if ! git rev-parse --verify --quiet "$BASE^{commit}" >/dev/null; then
  # A workflow that checked out only the head commit still has the base locally
  # once it has been fetched; the bare name usually means "origin/<name>".
  if git rev-parse --verify --quiet "origin/$BASE^{commit}" >/dev/null; then
    BASE="origin/$BASE"
  else
    echo "check-fmt-changed: base '$BASE' is not reachable; fetch it first (actions/checkout needs fetch-depth)" >&2
    exit 2
  fi
fi

MERGE_BASE=$(git merge-base "$BASE" HEAD)
REPO_ROOT=$(git rev-parse --show-toplevel)
WORK=$(mktemp -d)
trap 'rm -rf "$WORK"' EXIT

# New-side line ranges this change added, one "rel-path<TAB>start<TAB>end" per hunk.
git diff -U0 --no-color "$MERGE_BASE"...HEAD -- '*.rs' |
  awk '
    /^\+\+\+ b\// { file = substr($2, 3); next }
    /^@@/ {
      if (match($0, /\+[0-9]+(,[0-9]+)?/)) {
        spec = substr($0, RSTART + 1, RLENGTH - 1)
        split(spec, a, ",")
        start = a[1] + 0
        lines = (a[2] == "" ? 1 : a[2] + 0)
        if (lines > 0) print file "\t" start "\t" start + lines - 1
      }
    }
  ' >"$WORK/added.tsv"

if [ ! -s "$WORK/added.tsv" ]; then
  echo "check-fmt-changed: no Rust lines changed."
  exit 0
fi

# rustfmt follows `mod` declarations, so it is enough to hand it the entry point
# of each changed directory; the check output is filtered by path anyway.
while IFS= read -r rel; do
  [ -f "$REPO_ROOT/$rel" ] && printf '%s\0' "$REPO_ROOT/$rel"
done < <(cut -f1 "$WORK/added.tsv" | sort -u) >"$WORK/files.nul"

if [ ! -s "$WORK/files.nul" ]; then
  echo "check-fmt-changed: every changed Rust file was deleted."
  exit 0
fi

# `Diff in <abs path>:<line>:` is rustfmt's --check report. RUSTFMT_CMD is
# intentionally word-split so CI can pin a toolchain (`rustup run stable rustfmt`).
RUSTFMT_CMD=${RUSTFMT_CMD:-rustfmt}
# shellcheck disable=SC2086
xargs $RUSTFMT_CMD --edition 2021 --check <"$WORK/files.nul" >"$WORK/fmt.out" 2>&1 || true
awk '
  /^Diff in / {
    p = $3
    sub(/:$/, "", p)            # trailing colon, not the path/line separator
    line = p
    sub(/^.*:/, "", line)
    sub(/:[0-9]+$/, "", p)
    if (line ~ /^[0-9]+$/) print p "\t" line
  }
' "$WORK/fmt.out" >"$WORK/drift.tsv"

if [ ! -s "$WORK/drift.tsv" ]; then
  echo "check-fmt-changed: rustfmt is clean on the changed files."
  exit 0
fi

if awk -F'\t' -v root="$REPO_ROOT/" -v added="$WORK/added.tsv" '
  BEGIN {
    while ((getline a < added) > 0) {
      split(a, p, "\t")
      n = ++hunks[p[1]]
      from[p[1], n] = p[2] + 0
      to[p[1], n] = p[3] + 0
    }
  }
  {
    idx = index($1, root)
    if (idx != 1) next
    rel = substr($1, length(root) + 1)
    line = $2 + 0
    for (i = 1; i <= hunks[rel]; i++) {
      # A hunk header can sit a few context lines before the added text.
      if (line >= from[rel, i] - 4 && line <= to[rel, i] + 4) {
        print rel ":" line " would be reformatted by rustfmt"
        bad = 1
        break
      }
    }
  }
  END { exit bad ? 1 : 0 }
' "$WORK/drift.tsv"; then
  echo "check-fmt-changed: no formatting drift introduced by this change."
else
  echo "check-fmt-changed: run 'rustfmt --edition 2021 <file>' on the lines above." >&2
  exit 1
fi
