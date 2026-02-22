# Implementation Feedback — Post-Review

Review of Claude's `a791eca` commit and the homebrew-tools `fc1b7ce` commit.

## Issue 1: `cargo_bin()` deprecation in tests

**File:** `tests/cli_test.rs:11-13`

Claude used `#[allow(deprecated)]` on the `cargo_bin()` function because the replacement `cargo_bin!()` macro requires a string literal — it can't accept the runtime `bin_name()` value.

**Problem:** `cargo_bin()` is deprecated because it guesses `target/...` paths and breaks under custom `CARGO_TARGET_DIR` / `--build-dir` setups. It may be removed in a future major `assert_cmd` release.

**Fix:** Use `#[cfg]`-gated `cargo_bin!()` calls instead:

```rust
fn bin_cmd() -> assert_cmd::Command {
    #[cfg(feature = "stable")]
    { assert_cmd::Command::new(assert_cmd::cargo::cargo_bin!("git-forest")) }
    #[cfg(feature = "beta")]
    { assert_cmd::Command::new(assert_cmd::cargo::cargo_bin!("git-forest-beta")) }
}
```

The `bin_name()` helper can use the same `#[cfg]` pattern (it's used in test assertions too, not just for finding the binary).

**Severity:** Low. Current code works. Fix before next `assert_cmd` major bump.

## Issue 2: "multiple build targets" Cargo warning

**Symptom:** Every `cargo build/run/test` emits:
```
warning: file `src/main.rs` found to be present in multiple build targets:
  * `bin` target `git-forest`
  * `bin` target `git-forest-beta`
```

**Root cause:** Two `[[bin]]` entries in `Cargo.toml` point at the same `path = "src/main.rs"`. Cargo warns about this unconditionally — it doesn't reason about `required-features` mutual exclusivity or `compile_error!`.

**Is it harmful?** No. Only one binary is actually built per invocation (the one whose `required-features` are satisfied). The warning is cosmetic but noisy — it appears on every build, test, and clippy run.

**Fix:** Split into lib + two thin bin wrappers:

```
src/lib.rs          ← move all modules + main_entry() here
src/bin/git-forest.rs      ← fn main() { git_forest::main_entry(); }
src/bin/git-forest-beta.rs ← fn main() { git_forest::main_entry(); }
```

Update `[[bin]]` entries to point at `src/bin/git-forest.rs` and `src/bin/git-forest-beta.rs`. Each target has a unique path, warning disappears.

This is a common Rust pattern (bin wrappers + lib). The refactor is mechanical — move `mod` declarations and `main()`/`run()`/`output()` from `src/main.rs` to `src/lib.rs`, make entry point `pub`, create two one-line bin files.

**Severity:** Medium. Not a bug, but the warning on every build is a papercut. Worth fixing soon since the refactor is small.

## Homebrew formula review

**File:** `dliv/homebrew-tools/Formula/git-forest-beta.rb`

The formula looks correct:
- Placeholder version `0.0.0` and zeroed SHA256 — appropriate for a formula that hasn't had a real release yet
- Binary naming matches CI artifact naming: `git-forest-beta-aarch64-apple-darwin` → `git-forest-beta`
- Caveats correctly reference `git forest-beta`, `~/.config/git-forest-beta/config.toml`
- No `conflicts_with` — both formulae can coexist (matches design decision)
- Test block checks `git-forest-beta --version`

**One nit:** The caveats agentic workflows section (lines 32-39) still says `git forest` in the example AGENTS.md snippet. Users who only have the beta installed would be told to reference the stable binary name. Consider changing to `git forest-beta` or noting that the snippet should match whichever version is installed.

## Summary

| Item | Severity | Action |
|---|---|---|
| `cargo_bin()` deprecation | Low | Replace with `#[cfg]`-gated `cargo_bin!()` |
| "multiple build targets" warning | Medium | Refactor to lib + thin bin wrappers |
| Homebrew beta caveats snippet | Nit | Update agentic example to reference `git forest-beta` |
