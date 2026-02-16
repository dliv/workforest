# Phase Contracts 1 — `AbsolutePath` Newtype

**Branch:** `dliv/setup`
**Predecessor:** Phase 5B (94b3abc — multi-template config system)
**Baseline:** 164 tests (136 unit + 28 integration), all passing.

Goal: Introduce an `AbsolutePath` newtype that makes the "path must be absolute" invariant unrepresentable at compile time. This eliminates ~6 runtime `debug_assert!` calls and prevents an entire class of bug (relative path passed where absolute was expected).

---

## 1. The Newtype: `AbsolutePath`

### Definition (in `src/paths.rs`)

```rust
use std::ops::Deref;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct AbsolutePath(PathBuf);

impl AbsolutePath {
    /// Construct from a PathBuf that is already absolute.
    /// Returns None if not absolute.
    pub fn new(path: PathBuf) -> Option<Self> {
        if path.is_absolute() {
            Some(Self(path))
        } else {
            None
        }
    }

    /// Unwrap to inner PathBuf.
    pub fn into_inner(self) -> PathBuf {
        self.0
    }

    /// Join a relative component, returning a new AbsolutePath.
    /// Safe because absolute + relative = absolute.
    pub fn join<P: AsRef<Path>>(&self, path: P) -> AbsolutePath {
        AbsolutePath(self.0.join(path))
    }
}

impl Deref for AbsolutePath {
    type Target = Path;
    fn deref(&self) -> &Path {
        &self.0
    }
}

impl AsRef<Path> for AbsolutePath {
    fn as_ref(&self) -> &Path {
        &self.0
    }
}

impl std::fmt::Display for AbsolutePath {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0.display())
    }
}
```

### Serde Support

`AbsolutePath` needs `Serialize` and `Deserialize` for:
- `ResolvedTemplate.worktree_base` (written to config TOML via `write_config_atomic`)
- `ResolvedRepo.path` (written to config TOML)
- `RepoMeta.source` (written to `.forest-meta.toml`)
- JSON output structs (via `--json` flag)

Implement via serde's `TryFrom`-style pattern:

```rust
impl Serialize for AbsolutePath {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.0.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for AbsolutePath {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let path = PathBuf::deserialize(deserializer)?;
        AbsolutePath::new(path)
            .ok_or_else(|| serde::de::Error::custom("path must be absolute"))
    }
}
```

This means deserializing a non-absolute path from TOML/JSON fails at parse time — exactly the right behavior.

### `expand_tilde` Changes

```rust
// BEFORE
pub fn expand_tilde(path: &str) -> PathBuf

// AFTER
pub fn expand_tilde(path: &str) -> Result<AbsolutePath>
```

When `HOME` is unset and path starts with `~/`, the function currently returns the unexpanded path (a relative `PathBuf`). After the change, it returns `Err` — surfacing the problem as a user-facing error instead of silently producing a broken path.

The `debug_assert!` on line 20–23 (`result must not start with ~/`) is eliminated — the constructor guarantees it.

### `forest_dir` Changes

```rust
// BEFORE
pub fn forest_dir(worktree_base: &Path, name: &str) -> PathBuf

// AFTER
pub fn forest_dir(worktree_base: &AbsolutePath, name: &str) -> AbsolutePath
```

`AbsolutePath.join(relative)` returns `AbsolutePath`, so this is type-safe by construction.

---

## 2. File-by-File Changes

### `src/paths.rs` — Major Changes

| Item | Change |
|------|--------|
| New `AbsolutePath` struct | Add with `new()`, `into_inner()`, `join()`, `Deref`, `AsRef<Path>`, `Display`, `Serialize`, `Deserialize` |
| `expand_tilde()` | Return `Result<AbsolutePath>` instead of `PathBuf`. Error when HOME unset and path has `~/`. |
| `sanitize_forest_name()` | No change (returns `String`, not a path) |
| `forest_dir()` | Takes `&AbsolutePath`, returns `AbsolutePath` |
| `debug_assert!` (line 20–23) | **Remove** — constructor guarantees no `~/` prefix |
| Tests | Update `expand_tilde` tests to unwrap `Result`. Add tests for `AbsolutePath::new()` (absolute succeeds, relative fails). Add test for `expand_tilde` with unset HOME errors. |

### `src/config.rs` — Moderate Changes

| Item | Change |
|------|--------|
| `ResolvedTemplate.worktree_base` | `PathBuf` → `AbsolutePath` |
| `ResolvedRepo.path` | `PathBuf` → `AbsolutePath` |
| `parse_config()` | `expand_tilde()` calls now return `Result<AbsolutePath>` — propagate with `?` |
| `debug_assert!` (line 181–184) | **Remove** — `worktree_base` is `AbsolutePath` by construction |
| `debug_assert!` (line 185–198) | **Keep** — these check repo name invariants, not path invariants |
| `all_worktree_bases()` | Return `Vec<&Path>` still works (via `Deref`) |
| `write_config_atomic()` | `AbsolutePath` serializes as `PathBuf`, no change needed |
| `TemplateConfig` (raw TOML struct) | Keep as `PathBuf` — raw deserialization doesn't validate. Validation happens when resolving. |
| `RepoConfig` (raw TOML struct) | Keep as `PathBuf` — same reason |
| Tests | Update `ResolvedTemplate` construction to use `AbsolutePath::new(PathBuf::from("/...")).unwrap()`. Only affects tests that construct `ResolvedTemplate` directly (not those that go through `parse_config`). |

### `src/meta.rs` — Minor Changes

| Item | Change |
|------|--------|
| `RepoMeta.source` | `PathBuf` → `AbsolutePath` |
| `ForestMeta::read()` | Deserialization now validates paths are absolute (via `AbsolutePath`'s `Deserialize` impl) |
| Tests | Update `sample_meta()` and TOML literals — paths are already absolute, so no logic changes |

### `src/commands/init.rs` — Moderate Changes

| Item | Change |
|------|--------|
| `validate_init_inputs()` | `expand_tilde()` calls now return `Result<AbsolutePath>` — propagate with `?` |
| `debug_assert!` (lines 120–122) | **Remove** — `worktree_base` is `AbsolutePath` by construction |
| `debug_assert!` (lines 124–127) | **Keep** — repo name invariant, not path |
| `InitResult.worktree_base` | `PathBuf` → `AbsolutePath` |
| `InitRepoSummary.path` | `PathBuf` → `AbsolutePath` |
| Tests | Mechanical — paths in test helpers are already absolute |

### `src/commands/new.rs` — Moderate Changes

| Item | Change |
|------|--------|
| `ForestPlan.forest_dir` | `PathBuf` → `AbsolutePath` |
| `RepoPlan.source` | `PathBuf` → `AbsolutePath` |
| `RepoPlan.dest` | `PathBuf` → `AbsolutePath` |
| `NewResult.forest_dir` | `PathBuf` → `AbsolutePath` |
| `NewRepoResult.worktree_path` | `PathBuf` → `AbsolutePath` |
| `plan_forest()` | `forest_dir()` now returns `AbsolutePath`, flows naturally. `tmpl.worktree_base` is already `AbsolutePath`. |
| `execute_plan()` | Uses `plan.forest_dir` and `repo_plan.source` — both now `AbsolutePath`, `.as_ref()` works for `Path` args |
| Tests | `TestEnv.worktree_base()` returns `PathBuf` → update to return `AbsolutePath`. `TestEnv.repo_path()` → `AbsolutePath`. These cascade cleanly since test paths are under `/tmp/...` (absolute). |

### `src/commands/rm.rs` — Moderate Changes

| Item | Change |
|------|--------|
| `RmPlan.forest_dir` | `PathBuf` → `AbsolutePath` |
| `RepoRmPlan.worktree_path` | `PathBuf` → `AbsolutePath` |
| `RepoRmPlan.source` | `PathBuf` → `AbsolutePath` |
| `RmResult.forest_dir` | `PathBuf` → `AbsolutePath` |
| `plan_rm()` | `forest_dir.join(&repo.name)` → `AbsolutePath.join()` returns `AbsolutePath`, clean |
| `execute_rm()` / helpers | Use `.as_ref()` where `&Path` is needed |
| Tests | Mechanical — test paths are already absolute |

### `src/commands/exec.rs` — Minor Changes

| Item | Change |
|------|--------|
| `cmd_exec()` | `forest_dir: &Path` parameter. No change needed — `&AbsolutePath` derefs to `&Path`, and callers pass `&dir` where `dir: AbsolutePath`. Or: change param to `&AbsolutePath` for clarity. **Decision: keep as `&Path`** — this function doesn't need to enforce absoluteness, it's already guaranteed by the caller. |
| No struct changes | `ExecResult` has no path fields |

### `src/commands/status.rs` — Minor Changes

Same as `exec.rs` — `cmd_status` takes `&Path`, no struct changes needed.

### `src/commands/ls.rs` — No Changes

Works from `ForestMeta` (paths already `AbsolutePath` via deserialization) and `&[&Path]` slices (callers provide via `Deref`).

### `src/forest.rs` — No Changes

All functions take `&Path` parameters, which work with `&AbsolutePath` via `Deref`. Return types use `PathBuf` for the found forest directory — this comes from `entry.path()` (filesystem), not from our types. Leave as `PathBuf`.

### `src/main.rs` — Minor Changes

`resolve_forest_multi` returns `(PathBuf, ForestMeta)` — the `PathBuf` is a filesystem-discovered path, not one from our config. This is fine as `PathBuf`. The `&dir` passed to `cmd_exec`/`cmd_status`/`cmd_rm` auto-derefs.

### `src/testutil.rs` — Moderate Changes

| Item | Change |
|------|--------|
| `TestEnv::worktree_base()` | Return `AbsolutePath` (TempDir paths are absolute) |
| `TestEnv::repo_path()` | Return `AbsolutePath` |
| `TestEnv::create_repo()` | Return `AbsolutePath` |
| `TestEnv::create_repo_with_remote()` | Return `AbsolutePath` |
| `make_repo()` | `source` field becomes `AbsolutePath`. Change `PathBuf::from("/tmp/src/...")` to `AbsolutePath::new(PathBuf::from("/tmp/src/...")).unwrap()` |
| `setup_forest_with_git_repos()` | Return `(AbsolutePath, ForestMeta)` — or keep as `(PathBuf, ForestMeta)` since the path comes from `base.join()`. **Decision: keep as `(PathBuf, ForestMeta)` + wrap to `AbsolutePath` inside**, since `tempfile` returns absolute paths. |

### `tests/cli_test.rs` — No Changes Expected

Integration tests run `git forest` CLI commands and check stdout/stderr. They don't construct Rust types directly.

---

## 3. Assertions Eliminated

| # | Location | Assertion | Eliminated By |
|---|----------|-----------|---------------|
| 1 | `paths.rs:20–23` | `expand_tilde` result not start with `~/` | `expand_tilde` returns `Result<AbsolutePath>` — tilde expansion failure is now an error, and success guarantees absolute |
| 2 | `config.rs:181–184` | `worktree_base.is_absolute()` | Field is `AbsolutePath` — can't be non-absolute |
| 3 | `init.rs:120–122` | `worktree_base` absolute after validation | Field is `AbsolutePath` — can't be non-absolute |

**Kept (3 remaining):**
- `config.rs:185–188` — repo names non-empty (Phase Contracts 2 candidate)
- `config.rs:189–199` — repo names unique (no newtype candidate — set membership)
- `init.rs:124–127` — repo names non-empty (Phase Contracts 2 candidate)
- `paths.rs:31–34` — `sanitize_forest_name` result has no `/` (Phase Contracts 2 candidate)

---

## 4. Preconditions That Become Unnecessary

The following preconditions from the audit report are **eliminated by the type system** — no `debug_assert!` needed:

| Function | Would-Be Assertion | Why Unnecessary |
|----------|-------------------|-----------------|
| `plan_forest()` | `tmpl.worktree_base.is_absolute()` | Param is `AbsolutePath` |
| `plan_forest()` | `tmpl.repos.iter().all(\|r\| r.path.is_absolute())` | Field is `AbsolutePath` |
| `execute_plan()` | `plan.forest_dir.is_absolute()` | Field is `AbsolutePath` |
| `execute_plan()` | `plan.repo_plans.iter().all(\|r\| r.source.is_absolute())` | Field is `AbsolutePath` |
| `plan_rm()` / `execute_rm()` | `forest_dir.is_absolute()` | Caller passes `AbsolutePath` (or `&Path` derived from one) |

The **remaining precondition candidates** from the audit that still make sense as `debug_assert!`:

| Function | Assertion | Rationale |
|----------|-----------|-----------|
| `cmd_exec()` | `debug_assert!(forest_dir.is_absolute())` | Takes `&Path`, not `&AbsolutePath`. The type system doesn't enforce it at this boundary. |
| `cmd_status()` | `debug_assert!(forest_dir.is_absolute())` | Same — takes `&Path` |

**Decision: Add these 2 preconditions.** They're the only path-is-absolute checks that survive the newtype introduction.

---

## 5. Execution Order

Each step must pass `just check && just test`.

1. **`src/paths.rs`** — Add `AbsolutePath` struct with all impls. Change `expand_tilde` → `Result<AbsolutePath>`. Change `forest_dir` signature. Update tests. Remove `debug_assert!` on line 20–23.
2. **`src/config.rs`** — Change `ResolvedTemplate.worktree_base` and `ResolvedRepo.path` to `AbsolutePath`. Update `parse_config()`. Remove path `debug_assert!`. Update tests.
3. **`src/meta.rs`** — Change `RepoMeta.source` to `AbsolutePath`. Update tests.
4. **`src/testutil.rs`** — Update `TestEnv` return types and `make_repo()`.
5. **`src/commands/init.rs`** — Update `validate_init_inputs()`, remove path `debug_assert!`, update result types. Update tests.
6. **`src/commands/new.rs`** — Update plan/result struct fields. Update tests.
7. **`src/commands/rm.rs`** — Update plan/result struct fields. Update tests.
8. **`src/commands/exec.rs`** — Add `debug_assert!(forest_dir.is_absolute())` precondition.
9. **`src/commands/status.rs`** — Add `debug_assert!(forest_dir.is_absolute())` precondition.
10. **`src/main.rs`** — Any remaining compilation fixes.
11. **Run full suite** — `just check && just test`.

**Note:** Steps 1–4 must be sequential (each depends on the previous). Steps 5–7 can potentially be done in parallel after step 4, since they touch different files. Steps 8–9 are independent of each other.

---

## 6. Test Strategy

### New Tests

| Test | Location | What it verifies |
|------|----------|-----------------|
| `absolute_path_new_absolute` | `paths.rs` | `AbsolutePath::new("/foo")` returns `Some` |
| `absolute_path_new_relative` | `paths.rs` | `AbsolutePath::new("foo")` returns `None` |
| `absolute_path_join` | `paths.rs` | `.join("bar")` on `/foo` produces `/foo/bar` as `AbsolutePath` |
| `absolute_path_deref` | `paths.rs` | Can pass `&AbsolutePath` where `&Path` expected |
| `absolute_path_display` | `paths.rs` | `Display` shows the path string |
| `absolute_path_serde_round_trip` | `paths.rs` | Serialize then deserialize preserves value |
| `absolute_path_deserialize_relative_fails` | `paths.rs` | Deserializing `"foo/bar"` fails |
| `expand_tilde_returns_absolute_path` | `paths.rs` | Return type is `AbsolutePath` |

### Modified Tests

All existing tests that construct `ResolvedTemplate`, `ResolvedRepo`, `RepoMeta`, plan structs, or result structs with `PathBuf` fields need to wrap in `AbsolutePath::new(...).unwrap()`. These are mechanical changes. Approximately 30–40 test sites across config.rs, init.rs, new.rs, rm.rs, status.rs, ls.rs, testutil.rs.

---

## 7. Risk Assessment

**Low risk.** The change is mechanical:
- Every path in config and meta is already absolute in practice (created by `expand_tilde` or from filesystem traversal).
- The newtype just makes the compiler enforce what's already true.
- If something was secretly non-absolute, the `AbsolutePath::new().unwrap()` in tests will catch it immediately.
- The serde `Deserialize` impl catches non-absolute paths at parse time for config and meta files.

**One subtle point:** `forest.rs::resolve_forest_multi` returns `(PathBuf, ForestMeta)` where the `PathBuf` comes from `entry.path()`. On all platforms, `read_dir` on an absolute path produces absolute child paths. But this path doesn't go through `AbsolutePath` construction — it stays as `PathBuf` and gets passed to `cmd_rm`/`cmd_status`/`cmd_exec` as `&Path`. This is correct: those functions take `&Path` and we add `debug_assert!(is_absolute)` preconditions to catch any surprises.
