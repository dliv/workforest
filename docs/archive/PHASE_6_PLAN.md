# Phase 6 — Hardening

**Branch:** `dliv/setup`
**Predecessor:** Phase 5B (94b3abc — multi-template config system)
**Baseline:** 160 tests (133 unit + 27 integration), `just check` clean except one `dead_code` warning on `git_stream`.

Goal: clean up dead code, update documentation, audit error messages, cover edge cases, and tighten lints. Land as a single commit.

---

## 1. Dead code cleanup

### 1a. Remove `git_stream` (src/git.rs:59-69)

`git_stream` is never called from production code — only from its own unit tests. It was likely written for `exec` but `exec` ended up using `std::process::Command` directly with inherited stdio. Remove the function and its two tests (`git_stream_returns_exit_status`, `git_stream_returns_failure_status`). This eliminates the only clippy/rustc warning.

### 1b. Remove unused `TestEnv` methods (src/testutil.rs)

These four methods are defined but never called from any test:

| Method | Line | Action |
|---|---|---|
| `create_repo_with_branch` | 53 | Remove — no callers anywhere |
| `config_path` | 71 | Remove — no callers anywhere |
| `src_dir` | 75 | Remove — no callers anywhere |
| `default_config` | 161 | Remove — no callers (only referenced in archived docs). Also remove the `ResolvedConfig` import since it becomes unused. |

After removing `default_config`, the `BTreeMap` and `ResolvedConfig` imports in testutil.rs become unused — remove those too.

---

## 2. README update

Current README says "Phase 4 complete. 138 tests." Update to reflect Phase 5B:

- **Status section:** "Phase 5B complete. Multi-template config support. All core commands work: `init`, `new`, `rm`, `ls`, `status`, `exec`. 160+ tests." (use "160+" since we'll add tests in this phase)
- **Quick Start `init` example:** Add `--template` flag to show it's available (even though "default" is the default, showing it signals the feature exists).
- **`init` options table:**
  - Add `--template <name>` row: "Template name to create or update (default: default)"
  - Change `--force` description from "Overwrite existing config" to "Overwrite existing template by the same name"
- **`new` options table:**
  - Add `--template <name>` row: "Template to use (default: from config)"

---

## 3. Error message audit

Walk all `bail!()` and `eprintln!("error:` calls. Fix inconsistencies.

### Inconsistencies found:

| Location | Issue | Fix |
|---|---|---|
| main.rs:42 | `eprintln!("error: ...")` + `process::exit(1)` instead of `bail!()` for missing `--feature-branch-template`. Message uses `Hint:` (capitalized), while all `bail!()` messages use `\n  hint:` (lowercase, indented). | Refactor to use `bail!()` with lowercase `\n  hint:` pattern, remove the `process::exit(1)`. The `Option<String>` on `feature_branch_template` can become a required arg handled by clap instead. **However**, clap's built-in error for missing required args won't include our custom hint. Better approach: keep it as `bail!()` but use the standard hint format. |
| init.rs:40 | `"at least one --repo is required\nHint:"` — uses `\nHint:` (capital H, no indent) | Change to `\n  hint:` for consistency |
| init.rs:44 | `"--feature-branch-template must contain {name}"` — no hint | Add hint: `\n  hint: use a template like "yourname/{name}"` |
| init.rs:64 | `"repo path does not exist: ...\nHint:"` — capital H, no indent | Change to `\n  hint:` |
| init.rs:81 | `"not a git repository: ...\nHint:"` — capital H, no indent | Change to `\n  hint:` |
| init.rs:101 | `"duplicate repo name: ...\nHint:"` — capital H, no indent | Change to `\n  hint:` |
| config.rs:99 | `"config not found at ...\nRun \`git forest init\`..."` — starts with capital R, no `hint:` keyword | Change to `\n  hint: run \`git forest init\` to create one` |

All other `bail!()` calls already use the correct `\n  hint:` pattern.

### Also:
- exec.rs:14 `bail!("no command specified")` — no hint needed (clap would catch this in practice), leave as-is.
- exec.rs:25 `eprintln!("  warning: worktree missing...")` — fine as-is (informational).
- forest.rs:92 `anyhow!("forest '{}' not found", n)` — could use a hint. Add `\n  hint: run \`git forest ls\` to see available forests`.
- forest.rs:97 `anyhow!("not inside a forest directory")` — add hint: `\n  hint: specify a forest name, or cd into a forest directory`.

---

## 4. Edge case tests

### 4a. Config parsing edge cases (src/config.rs)

Add unit tests for:

- **Template with zero repos:** A `[template.empty]` section with `repos = []`. Currently this parses successfully but `plan_forest` would bail with "no repos configured". The config parser should reject this early with a clear error. Add validation in `parse_config`: `if tmpl_config.repos.is_empty() { bail!("template {:?}: must have at least one repo", tmpl_name) }`.
- **Valid TOML, wrong shape:** e.g., missing `[template]` section entirely, or `template` is a string instead of a table. Verify toml deserialization gives a reasonable error (it should, via serde). Add a test to confirm.
- **Empty template name in TOML:** `[template.""]` — currently parses OK (test exists at line 622). This is an acceptable edge case, no change needed.

### 4b. Forest name edge cases (src/commands/new.rs)

Add tests for:
- **Forest name that is only special characters:** e.g., `"////"` — sanitizes to `"----"`, should still work.
- **Forest name with spaces:** e.g., `"my feature"` — sanitizes to `"my feature"` (spaces aren't replaced). This actually works but is unusual. No validation needed, just document the behavior in a test.

---

## 5. Integration test: multi-template round-trip

Add a new integration test `multi_template_round_trip` to `tests/cli_test.rs`:

1. `init --template alpha` with repo foo-api
2. `init --template beta` with repo foo-web (different worktree_base)
3. `new alpha-feature --mode feature --template alpha --no-fetch`
4. `new beta-feature --mode feature --template beta --no-fetch`
5. `ls` → verify both forests appear
6. `rm alpha-feature`
7. `ls` → verify only beta-feature remains
8. `rm beta-feature`
9. `ls` → verify empty

This is the end-to-end multi-template test that's currently missing.

---

## 6. Clippy strictness

### Approach

Add `#![warn(clippy::all)]` to the top of `src/main.rs`. This is already the default, but making it explicit signals intent. Do NOT add `clippy::pedantic` — it's too noisy for this codebase (will trigger on things like `must_use` on every function, `missing_docs_in_private_items`, etc.).

Run `cargo clippy` after adding the annotation, fix any new warnings that arise, and `#[allow]` with comments for any that are false positives or intentional.

---

## Checklist (execution order)

1. [ ] Dead code cleanup (§1) — remove `git_stream`, remove 4 unused `TestEnv` methods
2. [ ] Error message audit (§3) — normalize all hint formats
3. [ ] Edge case validation + tests (§4) — add empty-repos-in-template validation, add edge case tests
4. [ ] Integration test (§5) — add multi-template round-trip
5. [ ] README update (§2) — update status, document `--template`
6. [ ] Clippy strictness (§6) — add `#![warn(clippy::all)]`, fix any new warnings
7. [ ] Run `just check && just test` — verify clean
8. [ ] Ask user about commit strategy
