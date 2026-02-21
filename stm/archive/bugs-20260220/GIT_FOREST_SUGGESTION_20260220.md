# git-forest: Suggestions

**Discovered:** 2026-02-20

---

## 1. Agent instructions could hint about stale base refs

**Type:** Documentation suggestion (agent-instructions output)
**Severity:** Low — nice-to-have, not a bug

## What happened

An AI agent created a review forest with `git forest new --mode review`, then diffed
the PR branch against the local base ref (not `origin/<base>`). The local base was stale —
missing a recently merged branch — so the diff included ~17 files that were already on
the remote base and not part of the PR. This produced a review with bogus findings about
code that wasn't in the PR.

## Why git-forest is tangentially involved

The `git forest agent-instructions` output includes a "Review a multi-repo PR" example in
the Common Patterns section. An agent following that workflow will naturally need to diff
after creating the forest. The instructions don't mention which ref to diff against, so
the agent used the local base branch.

## Suggestion

Add a one-liner hint to the "Review a multi-repo PR" common pattern, something like:

```
# Diff against origin/<base>, not local <base> — local may be stale
git diff origin/main...feature/the-pr-branch
```

This keeps it minimal (git-forest is a worktree tool, not a review tool) but catches the
most likely footgun for agents following the documented review workflow.

---

## 2. `git forest ls` table columns don't align

**Type:** Bug
**Severity:** Low — cosmetic
**Version:** 0.2.16

The NAME column header is padded but the values aren't, so columns misalign when forest names have different lengths:

```
NAME                      AGE        MODE      BRANCHES
review-foo  56m ago     review  forest/review-foo, feat/foo (2)
review-bar-long-name      1d ago     review  forest/review-bar-long-name, feat/bar, feat/bar-2
```

The header suggests a wide NAME column, but shorter names aren't padded to match, so AGE/MODE/BRANCHES shift left for those rows.

---

## 3. `git forest rm` leaves orphaned state on partial failure

**Type:** Bug
**Severity:** Medium
**Version:** 0.2.16

### What happened

`git forest rm review-foo` on a forest with 3 repos. Two repos were clean and removed
successfully. The third had dirty files and failed. However, git-forest removed the forest
from its internal state tracking despite the partial failure. Retrying with `--force` then
returned "forest not found", leaving an orphaned worktree directory that git-forest couldn't
help clean up.

### Repro

```
$ git forest rm my-forest
# repo-a: removed OK
# repo-b: removed OK
# repo-c: FAILED (dirty files)
# exit code 1, errors reported

$ git forest rm my-forest --force
# error: forest "my-forest" not found

$ git forest ls
# (empty — forest gone from state)

$ ls ~/worktrees/my-forest/
# repo-c/   ← orphaned, git-forest can't reach it
```

### Suggested fix

Validate the clean/dirty state of all repos *before* removing any of them. If any repo
has dirty files, fail early with a message listing the dirty repos — before touching
anything. `--force` bypasses the check and removes everything unconditionally.

This avoids partial state entirely: either all repos are clean and `rm` succeeds
atomically, or none are touched and the user gets a clear message to re-run with `--force`.
