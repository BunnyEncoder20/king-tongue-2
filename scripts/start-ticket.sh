#!/usr/bin/env bash
# Creates a worktree for one ticket, based on the integration branch tip.
#
# Usage: scripts/start-ticket.sh <ticket-number> <slug> [integration-branch]
#   e.g. scripts/start-ticket.sh 17 movable-anchors
#
# This exists because getting the base right by hand failed eleven times out of
# twelve on this project. Agent worktrees were created from a stale ref (an old
# `main`) rather than the integration tip, so every implementer had to notice
# and reset; twice an agent instead worked in the primary working directory and
# left it checked out on its own branch, which silently left the integration
# branch behind while the work reported done.
#
# So the base is computed here rather than trusted. The script refuses to run
# in a dirty tree and refuses to reuse an existing path, because both of those
# are how the primary-directory case started.

set -euo pipefail

if [ "$#" -lt 2 ]; then
    echo "usage: $0 <ticket-number> <slug> [integration-branch]" >&2
    exit 64
fi

ticket="$1"
slug="$2"
integration="${3:-integration/core-mechanics}"

case "$ticket" in
    '' | *[!0-9]*)
        echo "FAIL: ticket must be a number, got '$ticket'" >&2
        exit 64
        ;;
esac

repo_root=$(git rev-parse --show-toplevel)
cd "$repo_root"

if [ -n "$(git status --porcelain)" ]; then
    echo "FAIL: the working tree is dirty. Commit or stash first." >&2
    echo "A worktree created from a dirty tree is how work gets stranded." >&2
    exit 1
fi

branch="ticket/${ticket}-${slug}"
path=".claude/worktrees/${branch//\//-}"

if [ -e "$path" ]; then
    echo "FAIL: $path already exists. Remove it first:" >&2
    echo "  git worktree remove $path" >&2
    exit 1
fi

if git show-ref --quiet --verify "refs/heads/$branch"; then
    echo "FAIL: branch $branch already exists." >&2
    exit 1
fi

echo "Fetching origin, so the base is the real tip and not a cached one."
git fetch --quiet origin

base="origin/$integration"
if ! git rev-parse --quiet --verify "$base" >/dev/null; then
    echo "FAIL: $base does not exist. Push the integration branch first," >&2
    echo "or pass a different one as the third argument." >&2
    exit 1
fi

base_sha=$(git rev-parse --short "$base")

git worktree add -b "$branch" "$path" "$base"

echo
echo "Worktree ready."
echo "  branch:  $branch"
echo "  path:    $path"
echo "  base:    $base at $base_sha"
echo
echo "Verify the base before writing code:"
echo "  git -C $path log --oneline -1    # expect $base_sha"
echo
echo "Read CODING_STANDARDS.md before the first commit: every new test is"
echo "mutation-checked, and commits land as the work goes rather than at the end."
