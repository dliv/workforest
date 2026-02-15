# Phase 5A Plan — Simplify Branch Config

**STATUS: PLANNED** — Ready for execution.

## Goal

Remove `username` as a separate config field. Rename `branch_template` to `feature_branch_template`. The template becomes self-contained: the user bakes their identity directly into it (e.g., `"dliv/{name}"` instead of `"{user}/{name}"` with a separate `--username dliv`).

Review mode branches (`forest/{name}`) remain hardcoded — no template needed.

## Motivation

`--username` is hidden state that exists only to fill `{user}` in the branch template. This is:
- One extra required flag on `init` that isn't obvious why it's needed.
- A level of indirection: to understand what branch name you'll get, you need to mentally compose two fields.
- A blocker for multi-template config (Phase 5B): each template would need its own username, which doesn't make sense.

After this change, what you see is what you get: `feature_branch_template = "dliv/{name}"` directly tells you the branch pattern.

## Summary of Changes

### Conceptual Changes

| Before | After |
|--------|-------|
| `branch_template = "{user}/{name}"` + `username = "dliv"` | `feature_branch_template = "dliv/{name}"` |
| `compute_target_branch(... branch_template, username ...)` | `compute_target_branch(... feature_branch_template ...)` |
| `--username dliv --branch-template "{user}/{name}"` | `--feature-branch-template "dliv/{name}"` |
| `{user}` and `{name}` placeholders | `{name}` is the only placeholder |

### What Doesn't Change

- Review mode branch computation: `forest/{name}` — hardcoded, no template.
- Forest meta format (`.forest-meta.toml`) — unaffected, stores resolved branch names.
- All commands except `init` and `new` — they read from meta, not config.
- `ResolvedRepo`, `RepoConfig`, `RepoMeta` — none of these contain username or branch_template.

---

## File-by-File Changes

### 1. `src/config.rs`

**Struct `GeneralConfig`** (line 15–20):

```rust
// BEFORE
pub struct GeneralConfig {
    pub worktree_base: PathBuf,
    pub base_branch: String,
    pub branch_template: String,
    pub username: String,
}

// AFTER
pub struct GeneralConfig {
    pub worktree_base: PathBuf,
    pub base_branch: String,
    pub feature_branch_template: String,
}
```

**`parse_config()`** (line 70–140):
- Line 75: Change validation from `raw.general.branch_template.contains("{name}")` to `raw.general.feature_branch_template.contains("{name}")`.
- Line 76: Update error message from `"branch_template must contain {name}"` to `"feature_branch_template must contain {name}"`.
- Lines 79–84: Remove `username` from `GeneralConfig` construction. Rename `branch_template` to `feature_branch_template`.

**`write_config_atomic()`** (line 142–178): No logic changes needed — it serializes `GeneralConfig` directly, so the struct change propagates automatically.

**Tests** (lines 181–379) — 9 test functions to update:

| Test | Change |
|------|--------|
| `parse_full_config` (line 185) | TOML: `branch_template` → `feature_branch_template`, remove `username` line. Remove `username` assert (line 209). Update template value from `"{user}/{name}"` to `"dliv/{name}"`. |
| `parse_minimal_config_defaults_applied` (line 218) | TOML: same field renames + remove `username`. |
| `tilde_expansion_on_worktree_base` (line 236) | TOML: same field renames + remove `username`. |
| `tilde_expansion_on_repo_path` (line 256) | TOML: same field renames + remove `username`. |
| `name_derived_from_path_when_omitted` (line 276) | TOML: same field renames + remove `username`. |
| `base_branch_inherited_from_general` (line 292) | TOML: same field renames + remove `username`. |
| `remote_defaults_to_origin` (line 314) | TOML: same field renames + remove `username`. |
| `duplicate_repo_names_error` (line 329) | TOML: same field renames + remove `username`. |
| `branch_template_must_contain_name` (line 364) | TOML: rename field to `feature_branch_template`. Remove `username` line. Update template value to something without `{name}` (e.g., `"dliv/feature"`). Update assertion to check for `"feature_branch_template"`. |

**Pattern for all config TOML in tests** — replace:
```toml
branch_template = "{user}/{name}"
username = "dliv"
```
with:
```toml
feature_branch_template = "dliv/{name}"
```

### 2. `src/commands/new.rs`

**`compute_target_branch()`** (lines 70–96):

```rust
// BEFORE
fn compute_target_branch(
    repo_name: &str,
    forest_name: &str,
    mode: &ForestMode,
    branch_template: &str,
    username: &str,
    branch_override: &Option<String>,
    repo_branches: &[(String, String)],
) -> String {
    // ...
    match mode {
        ForestMode::Feature => branch_template
            .replace("{user}", username)
            .replace("{name}", forest_name),
        ForestMode::Review => format!("forest/{}", forest_name),
    }
}

// AFTER
fn compute_target_branch(
    repo_name: &str,
    forest_name: &str,
    mode: &ForestMode,
    feature_branch_template: &str,
    branch_override: &Option<String>,
    repo_branches: &[(String, String)],
) -> String {
    // ...
    match mode {
        ForestMode::Feature => feature_branch_template
            .replace("{name}", forest_name),
        ForestMode::Review => format!("forest/{}", forest_name),
    }
}
```

**Call site in `plan_forest()`** (lines 222–230):

```rust
// BEFORE
let branch = compute_target_branch(
    &repo.name,
    &inputs.name,
    &inputs.mode,
    &config.general.branch_template,
    &config.general.username,
    &inputs.branch_override,
    &inputs.repo_branches,
);

// AFTER
let branch = compute_target_branch(
    &repo.name,
    &inputs.name,
    &inputs.mode,
    &config.general.feature_branch_template,
    &inputs.branch_override,
    &inputs.repo_branches,
);
```

**Tests** (lines 428–950) — updates needed:

| Test | Change |
|------|--------|
| `feature_mode_uses_branch_template` (line 453) | Remove `username` param from call. Change template from `"{user}/{name}"` to `"dliv/{name}"`. Rename test to `feature_mode_uses_feature_branch_template`. Expected result stays `"dliv/java-84/refactor-auth"`. |
| `review_mode_uses_forest_prefix` (line 467) | Remove `username` param. Template value doesn't matter (review mode ignores it) but update for consistency. |
| `branch_override_applies_to_all_repos` (line 481) | Remove `username` param from both calls. Update template for consistency. |
| `repo_branch_override_applies_to_specific_repo` (line 506) | Remove `username` param from both calls. Update template for consistency. |
| `plan_empty_config_repos_errors` (line 582) | Remove `username` from `GeneralConfig` construction. Update `branch_template` → `feature_branch_template`, value from `"{user}/{name}"` to `"testuser/{name}"`. |
| `plan_source_repo_missing_errors` (line 604) | Same GeneralConfig update. |
| `plan_feature_mode_all_repos` (line 778) | Expected branch changes from `"testuser/java-84/refactor-auth"` to match whatever `default_config` produces. Since `TestEnv::default_config` will change to `feature_branch_template: "testuser/{name}"`, result stays `"testuser/java-84/refactor-auth"`. No change needed to assertions. |

### 3. `src/commands/init.rs`

**`InitInputs` struct** (lines 9–15):

```rust
// BEFORE
pub struct InitInputs {
    pub worktree_base: String,
    pub base_branch: String,
    pub branch_template: String,
    pub username: String,
    pub repos: Vec<RepoInput>,
}

// AFTER
pub struct InitInputs {
    pub worktree_base: String,
    pub base_branch: String,
    pub feature_branch_template: String,
    pub repos: Vec<RepoInput>,
}
```

**`validate_init_inputs()`** (lines 37–134):
- Lines 38–40: Remove the `inputs.username.is_empty()` check entirely.
- Line 46: Change `inputs.branch_template` to `inputs.feature_branch_template`.
- Line 47: Update error message to `"--feature-branch-template must contain {name}"`.
- Lines 125–133: Remove `username` from `GeneralConfig` construction. Rename `branch_template` to `feature_branch_template`.

**Tests** (lines 176–419) — updates needed:

| Test | Change |
|------|--------|
| `make_init_inputs` helper (line 199) | Remove `username` field. Rename `branch_template` to `feature_branch_template`, value from `"{user}/{name}"` to `"testuser/{name}"`. |
| `validate_init_valid_inputs` (line 210) | Remove `username` assertion (line 224). |
| `validate_init_missing_username` (line 228) | **DELETE this entire test** — username no longer exists. |
| `validate_init_branch_template_missing_name` (line 299) | Update field name to `feature_branch_template`. Update value to `"dliv/feature"`. Update assertion to check for `"feature-branch-template"`. |
| `validate_init_tilde_expansion` (line 321) | Remove `username` field. Rename `branch_template` to `feature_branch_template`, update value. |
| All tests using `make_init_inputs` | Automatically fixed by the helper change. |

### 4. `src/cli.rs`

**`Command::Init` variant** (lines 17–42):

```rust
// BEFORE
        /// Branch naming template (must contain {name})
        #[arg(long, default_value = "{user}/{name}")]
        branch_template: String,
        /// Your username for branch templates
        #[arg(long)]
        username: Option<String>,

// AFTER
        /// Feature branch naming template (must contain {name}, e.g. "yourname/{name}")
        #[arg(long)]
        feature_branch_template: Option<String>,
```

**`--feature-branch-template` is required (no default value).** A bare `{name}` default would produce branches like `my-feature` with no user prefix, causing collisions with other developers. Making the flag required forces the user to consciously choose their pattern. Net effect vs. today: one flag (`--feature-branch-template "dliv/{name}"`) instead of two (`--username dliv --branch-template "{user}/{name}"`). Still a UX improvement.

The flag is `Option<String>` in clap so we can produce a clear error message in `main.rs` rather than clap's generic "required argument" message:

```
error: --feature-branch-template is required
  hint: git forest init --feature-branch-template "yourname/{name}" --repo <path>
```

### 5. `src/main.rs`

**`run()` function, `Command::Init` arm** (lines 25–98):

- Remove `username` from destructuring (line 29).
- Remove the `username.unwrap_or_else(...)` block (lines 41–44).
- Replace it with a similar block for `feature_branch_template`:
  ```rust
  let feature_branch_template = feature_branch_template.unwrap_or_else(|| {
      eprintln!("error: --feature-branch-template is required\n  hint: git forest init --feature-branch-template \"yourname/{{name}}\" --repo <path>");
      std::process::exit(1);
  });
  ```
- Remove `username` from `InitInputs` construction (line 92).
- Rename `branch_template` to `feature_branch_template` in destructuring (line 28) and `InitInputs` construction (line 91).

### 6. `src/testutil.rs`

**`TestEnv::default_config()`** (lines 157–177):

```rust
// BEFORE
        ResolvedConfig {
            general: GeneralConfig {
                worktree_base: self.worktree_base(),
                base_branch: "main".to_string(),
                branch_template: "{user}/{name}".to_string(),
                username: "testuser".to_string(),
            },
            repos,
        }

// AFTER
        ResolvedConfig {
            general: GeneralConfig {
                worktree_base: self.worktree_base(),
                base_branch: "main".to_string(),
                feature_branch_template: "testuser/{name}".to_string(),
            },
            repos,
        }
```

This preserves the same branch output (`testuser/<forest-name>`) so existing test assertions on branch names don't need to change.

### 7. `tests/cli_test.rs`

**Integration tests:**

| Test | Change |
|------|--------|
| `init_without_username_shows_hint` (line 12) | **REPLACE** with `init_without_feature_branch_template_shows_hint`: verify that `init --repo <path>` without `--feature-branch-template` fails with a hint mentioning the flag. |
| `init_creates_config` (line 47) | Remove `"--username", "testuser"` from args (lines 66–67). Add `"--feature-branch-template", "testuser/{name}"` to retain equivalent behavior. |
| `init_force_overwrites` (line 81) | Remove `"--username", "testuser"` from args. Add `"--feature-branch-template", "testuser/{name}"`. |
| `init_json_output` (line 117) | Remove `"--username", "testuser"` from args. Add `"--feature-branch-template", "testuser/{name}"`. |
| `setup_new_env` helper (line 257) | Remove `"--username", "testuser"` from init args. Add `"--feature-branch-template", "testuser/{name}"`. |

### 8. Documentation

| File | Change |
|------|--------|
| `README.md` | Update Quick Start example (remove `--username dliv`). Update `init` command reference: remove `--username`, rename `--branch-template` to `--feature-branch-template`. Update feature mode description from `{user}/{name}` to reference the template directly. |
| `docs/architecture-decisions.md` | Update the example config in Section 1 (lines 29–34): remove `username`, rename `branch_template` to `feature_branch_template`, update value. Add a note that Phase 5A is complete. |

---

## Execution Checklist

The implementing agent should execute in this order:

1. **`src/config.rs`** — Update `GeneralConfig` struct and `parse_config()`. Update all 9 tests.
2. **`src/commands/init.rs`** — Update `InitInputs`, `validate_init_inputs()`. Update tests, delete `validate_init_missing_username`.
3. **`src/testutil.rs`** — Update `default_config()`.
4. **`src/commands/new.rs`** — Update `compute_target_branch()` signature and body, update call site in `plan_forest()`. Update all test calls.
5. **`src/cli.rs`** — Update `Command::Init` variant.
6. **`src/main.rs`** — Update `Command::Init` destructuring and `InitInputs` construction.
7. **`tests/cli_test.rs`** — Update integration tests.
8. **`just check`** — Verify fmt + clippy clean.
9. **`just test`** — Verify all tests pass (should be 137 = 138 - 1 deleted test).
10. **Documentation** — Update `README.md` and `docs/architecture-decisions.md`.

## Migration Notes

This is a breaking change to the config file format. Existing `config.toml` files with `branch_template` and `username` will fail to parse. No migration code needed — the tool is pre-v1 with a single user. Just delete the old config and re-run `git forest init`.

## Test Impact

- **1 test deleted:** `validate_init_missing_username`
- **1 test replaced:** `init_without_username_shows_hint` → `init_without_feature_branch_template_shows_hint`
- **~25 tests modified:** Field renames, removal of username params, updated TOML strings.
- **Net test count:** 138 (same — 1 deleted, 1 replaced).

Land 5A as a single commit. Splitting across incremental commits creates a window where the code doesn't compile since the struct changes are interdependent.
