# Architecture rules and their gates

git-forest's architecture decisions live in [`docs/decisions/`](decisions/ADR_INDEX.md). This document says which of those rules a command enforces, where each rule is enforced, and what the enforcement does not prove.

One local command is the gate: `just check`. It runs, in order, `cargo fmt --check`, Clippy for both build channels (`stable` and `beta`) across the whole workspace, all tests for both channels including the policy tests in `crates/policy`, and the positive control in `scripts/policy-control.sh`. CI runs the same `just check`, so the local gate and CI cannot drift. Run it before every commit and before handing work back.

Rules come in three kinds. A **gated** rule fails `just check` when code breaks it. A **checkable** rule is verified by a reviewer with a search, because a mechanical check would cost more than it saves. A **judgment** rule needs a person to decide; this document states the criteria.

| Rule | Kind | Home |
|------|------|------|
| Environment, arguments, working directory, executable path, exit, process spawning and the clock are read only at sanctioned sites | Gated | Root `clippy.toml` (`disallowed-methods`); `#[expect]` at each sanctioned site |
| Only the shell prints | Gated | `[lints.clippy]` in the root `Cargo.toml` (`print_stdout`, `print_stderr`); `#[expect]` at each sanctioned site |
| Every file under `src/` has exactly one module class | Gated | `crates/policy/src/classification.rs`; `crates/policy/tests/classification.rs` |
| Crate-internal imports follow the class direction | Gated | `crates/policy/tests/direction.rs`; `ALLOWED_EDGES` |
| Pure modules contain no ambient effects | Gated | `crates/policy/tests/ambient.rs` |
| Production code declares no traits | Gated | `crates/policy/tests/traits.rs` |
| The bans still fire at every sanctioned site, and every ban path resolves | Gated | `scripts/policy-control.sh`; `crates/policy/control-expected.txt` |
| `new` and `rm` plan before they execute (ADR 0003) | Checkable | Search for `plan_forest`/`execute_plan` and `plan_rm`/`execute_rm` |
| Newtypes are validated once, at construction | Checkable | Search `src/paths.rs` for `pub fn new` on each newtype |
| When a module may become Pure | Judgment | Section "Pure modules" |
| When a trait earns its place | Judgment | Section "No production traits" |

An "ambient effect" is anything that reads or changes the outside world without being passed in: environment variables, process arguments, the working directory, the clock, the filesystem, processes, threads, the network or standard input.

## Module classes

Every `.rs` file under `src/` has exactly one class, listed in `MODULES` in `crates/policy/src/classification.rs`. The classification test fails when a file is missing from that list, listed twice, or listed but gone.

- **Root**: `src/lib.rs` and the binaries in `src/bin/`. The composition root: it reads process arguments, loads configuration, dispatches commands, prints results and decides exit codes. Root may import anything.
- **Shell**: `src/version_check.rs`, a self-contained update-check subsystem with its own effects (an HTTP call, a state file, a spawned background process). Shell imports Shared and Pure.
- **Command**: every file in `src/commands/`. Commands return typed results and do not print (ADR 0002). A command imports Shared, Pure and allowlisted Command modules.
- **Shared**: `src/config.rs`, `src/forest.rs`, `src/git.rs`, `src/meta.rs`, `src/paths.rs`. Types and helpers used across commands. Shared imports Shared and Pure.
- **Pure**: `src/channel.rs`, `src/cli.rs`. No ambient effects in their own source. Pure imports only Pure, plus allowlisted edges.
- **TestSupport**: `src/testutil.rs`, compiled only for tests. It is classified so that every file has a class, but it is not scanned.

Nothing except Root imports Root.

## Import direction

`crates/policy/tests/direction.rs` computes, for every module except TestSupport, the crate-internal modules it refers to, and checks each edge against the class rules above. An edge is any path that starts with `crate::`, `super::` or `self::` in the module's production code, written inline or inside a `use` tree (`use super::rm::{self, X}` counts as `super::rm`). `super` and `self` resolve against the module's own path, and each path is cut back to the longest known module, so `crate::meta::read` is an edge to `meta`. Visibility qualifiers such as `pub(crate)` and `pub(in crate::commands)` are not edges. Code inside a `#[cfg(test)] mod tests { ... }` block is not production code and is skipped; so are comments and string literals. An edge to a module's own child is allowed.

Edges the class rules refuse but that are sanctioned are listed in `ALLOWED_EDGES`, each with a reason:

- `commands::ls`, `commands::status`, `commands::rm` and `commands::reset` import `commands::branch_state`, the helper shared by the commands that inspect worktree branches.
- `commands::reset` imports `commands::rm`, because reset reuses rm's removal plan.
- `cli` imports `meta` and `paths` only for the types `ForestMode` and `DisposableRootEntry`.

An allowlist entry whose edge no longer exists fails the test too, so the list cannot go stale. A violation message names the edge, both classes and the rule, and says how to fix it: move the import, or add an `ALLOWED_EDGES` entry with a reason.

## Pure modules

A Pure module's production code may not contain an ambient effect. `crates/policy/tests/ambient.rs` scans each Pure module with a token-level scanner, so comments, strings and raw strings can neither hide nor fake a path, and aliases (`use std::env as e`), grouped imports, glob imports and raw identifiers are resolved. It reports:

- any use of `std::fs`, `std::env`, `std::process`, `std::net`, `std::time` or `std::thread`, inline or imported;
- standard input (`std::io::stdin` or `std::io::Stdin`);
- `env!` unless its only argument is a string literal starting with `CARGO_PKG_` (Cargo sets those from the manifest, so they are fixed per build; `src/cli.rs` uses `env!("CARGO_PKG_VERSION")`), and every `option_env!`;
- `include!`, `include_str!`, `include_bytes!` and `#[path = ...]`, which pull in text the scanner does not see;
- `unsafe`, `extern`, `macro_rules!`, glob imports of `std` or `core`, importing or aliasing `std` itself, and non-ASCII identifiers.

`std::path` is allowed: `PathBuf` is data, not an effect.

When may a module become Pure? When its own production code has none of the effects above and it imports only Pure modules, or holds an allowlisted edge with a reason naming how that edge goes away. `src/paths.rs` is Shared, not Pure, because `expand_tilde` reads `HOME`; it can become Pure once the home directory is passed in. Reclassifying a module is a one-line change in `MODULES`; the ambient and direction tests then say whether it holds.

## No production traits

ADR 0002 keeps the call graph shallow: no traits, dependency injection or ports-and-adapters machinery. `crates/policy/tests/traits.rs` fails when production code declares a trait (`trait` followed by a name, outside test modules). Implementing a trait from another crate, such as `impl std::fmt::Display for X`, is fine.

When does a trait earn its place? Only when production code has more than one real implementation that callers must treat the same way. Test doubles are not a reason, because tests run against real git repositories (ADR 0007). Adding one is a design decision: record it in an ADR and add an entry to `ALLOWED_TRAITS` in `crates/policy/tests/traits.rs` with the reason. An entry whose trait no longer exists fails the test.

## Ambient reads

The root `clippy.toml` bans, through Clippy's `disallowed-methods`, the functions that read or change the process's surroundings: `std::env::var`, `var_os`, `vars`, `vars_os`, `args`, `args_os`, `current_dir`, `set_current_dir`, `current_exe`, `home_dir`, `set_var` and `remove_var`; `std::process::exit`, `abort` and `Command::new`; and the clocks `chrono::Utc::now`, `chrono::Local::now`, `std::time::SystemTime::now` and `std::time::Instant::now`. Each entry carries a reason saying where the call belongs, and Clippy prints it with the error. Clippy resolves paths by definition, so aliases, glob imports and re-exports are caught.

A sanctioned site carries `#[expect(clippy::disallowed_methods, reason = "...")]` on the enclosing function (or, for `src/version_check.rs`, one `#![expect(...)]` for the whole module). `#[expect]` is used instead of `#[allow]` because a stale `#[expect]` fails the build with "this lint expectation is unfulfilled": when the call goes away, the attribute must go too. The sanctioned sites today are the composition root in `src/lib.rs` (`main_entry` and `run`), the update check in `src/version_check.rs`, the one git wrapper in `src/git.rs`, `exec`'s spawn of the user's command, and the reads below the root listed in "Planned follow-ups".

Tests are exempt. Unit tests inside `src/` get a crate-level `#![cfg_attr(test, allow(clippy::disallowed_methods, ...))]` in `src/lib.rs`; the integration tests get a crate-root `#![allow(...)]` in `tests/cli_test.rs`. Both build real git repositories and read the environment by design. Production code stays linted because Clippy also checks the library in its normal, non-test build, where the `cfg_attr` does not apply.

`clippy.toml` must stay the only one in the workspace: a per-crate `clippy.toml` replaces the root file for that crate instead of extending it.

## Printing

Commands return data and `src/lib.rs` prints it (ADR 0002). The root package's `[lints.clippy]` table turns on `print_stdout` and `print_stderr`, and `just check` runs Clippy with `-D warnings`, so `println!`, `print!`, `eprintln!` and `eprint!` fail outside sanctioned sites. Those sites carry `#[expect(clippy::print_stdout, ...)]` or `#[expect(clippy::print_stderr, ...)]`: `main_entry`, `run` and `output` in `src/lib.rs`, `src/version_check.rs`, and `cmd_exec` in `src/commands/exec.rs` (see "Planned follow-ups"). `allow-print-in-tests = true` in `clippy.toml` exempts test code.

## Positive control

A ban that silently stops firing looks exactly like a codebase with no violations. `scripts/policy-control.sh` (run by `just policy-control`, part of `just check`) proves the bans still work. It runs Clippy on the library and binaries for the stable channel with `--force-warn` on `clippy::disallowed_methods`, `clippy::print_stdout` and `clippy::print_stderr`, which makes every sanctioned site warn even though its `#[expect]` silences it in the normal build. It reduces each warning to its file and message, counts them, and compares the result with `crates/policy/control-expected.txt`; any difference fails with a diff. It also fails if Clippy reports that a `clippy.toml` path does not refer to a reachable function, so a typo in an entry cannot disable it silently.

The script forces `CARGO_TERM_COLOR=never` (CI sets `always`, and colour codes would hide every warning from the parser) and `LC_ALL=C` (so the sorted counts match on macOS and Ubuntu).

## Planned follow-ups

Every exemption and allowlist reason names the change that removes it. Together they are the to-do list the build enforces:

- **Exit codes move into `main_entry`.** `run` in `src/lib.rs` calls `std::process::exit` for some commands; typed failures will let `main_entry` own every exit code.
- **One clock reading, passed in.** `summarize_forest` (`ls`), `execute_plan` (`new`) and `backup_file` (`reset`) read the clock inside a command; `src/lib.rs` will read it once and pass the time in.
- **Environment, home and working directory, passed in.** `xdg_dir` in `src/config.rs` reads XDG variables, `expand_tilde` in `src/paths.rs` reads `HOME`, and `current_dir_preserving_symlinks` in `src/forest.rs` reads the working directory and `PWD`; `src/lib.rs` will read these and pass them in. After the `HOME` read moves, `src/paths.rs` can become Pure.
- **`exec`'s progress moves to a callback.** `cmd_exec` prints progress to stderr, which breaks ADR 0002; it will report progress through a callback as `rm` does. Spawning the user's command stays, by design.
- **`init`'s git check goes through `src/git.rs`.** `validate_init_inputs` runs `git` directly.
- **`ForestMode` and `DisposableRootEntry` move into a Pure module**, which removes the `cli -> meta` and `cli -> paths` allowlist entries.
- **Open question: where `version_check` belongs** in the module structure. It is classified Shell and exempted as a whole module until that is decided.
- **Open question: whether `rm`'s removal plan becomes a shared module**, which would replace the `commands::reset -> commands::rm` allowlist entry.

## Changing the rules

**Adding a module.** Create the file and add it to `MODULES` in `crates/policy/src/classification.rs` with its class. The classification test fails until you do. If the new module needs an import the class rules refuse, either restructure or add an `ALLOWED_EDGES` entry whose reason names who removes it.

**Adding an exemption.** Put `#[expect(<lint>, reason = "...")]` on the narrowest enclosing item, listing only the lints that actually fire there, with a reason that names the follow-up removing it. Then update `crates/policy/control-expected.txt`: the positive control fails with a diff showing the new count, which is the line to change. Add the follow-up to "Planned follow-ups".

**Adding a banned method.** Add an entry to `disallowed-methods` in the root `clippy.toml` with a reason saying where the call belongs. If the method has sanctioned call sites, exempt them as above; their lines in `crates/policy/control-expected.txt` prove the path resolves. If it has no call site anywhere, nothing proves the entry works: record that in "What the gates do not prove".

**Adding a trait or an allowed edge.** See "No production traits" and "Import direction". Both allowlists reject entries that are no longer used.

## What the gates do not prove

- Bare child-module paths are not followed by the direction scan: a path that starts with a child module's name, such as `init::x` or the `pub use rm::*;` re-exports in `src/commands/mod.rs`, is ignored. Only `crate::`, `super::` and `self::` paths are checked.
- Macro-generated code and text pulled in with `include!` are not scanned outside Pure modules; the scanner reads source tokens, not expanded code. In Pure modules, `macro_rules!` and the `include` macros are refused outright.
- Network calls are not banned: the `ureq` constructors are not in `clippy.toml`, because `src/version_check.rs` is the only network user. Filesystem access is banned only in Pure modules; commands read and write forests by design.
- The policy scans read only files under `src/`; integration tests, the policy crate itself and the `worker/` directory are not classified.
- A `clippy.toml` entry with no call site anywhere is covered only by Clippy's own "does not refer to a reachable function" warning. Judging from Clippy's source, that warning is not emitted when the path's first segment names a crate that is not loaded, so such an entry could do nothing unnoticed. This has not been verified.
- The Rust toolchain is not pinned. A future stable Clippy may add lints that fail `-D warnings`, as it already could before these gates existed.
- CI installs `just` from the Ubuntu archive on an `ubuntu-24.04` runner; that step was untested when the gates were introduced, and only a push or pull request exercises it.
