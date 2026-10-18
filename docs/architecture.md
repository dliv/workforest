# Architecture rules and their gates

git-forest's architecture decisions live in [`docs/decisions/`](decisions/ADR_INDEX.md). This document says which of those rules a command enforces, where each rule is enforced, and what the enforcement does not prove.

One local command is the gate: `just check`. It runs, in order, `cargo fmt --check`, Clippy for both build channels (`stable` and `beta`) across the whole workspace, all tests for both channels including the policy tests in `crates/policy`, and the positive control in `scripts/policy-control.sh`. CI runs the same `just check`, so the local gate and CI cannot drift. Run it before every commit and before handing work back.

Rules come in three kinds. A **gated** rule fails `just check` when code breaks it. A **checkable** rule is verified by a reviewer with a search, because a mechanical check would cost more than it saves. A **judgment** rule needs a person to decide; this document states the criteria.

The gates exist to catch architectural drift in ordinary code: a person or an agent writing a print in a command, a clock read in a shared module or an import against the class direction without noticing the rule. They are not a defense against deliberate evasion. Each check stays as small as the drift it catches, so an unusual spelling that ordinary code does not use and that slips past a check is listed under "What the gates do not prove" rather than closed with more machinery. A check that refuses ordinary, correct code is a bug to fix.

| Rule | Kind | Home |
|------|------|------|
| Environment, arguments, working directory, executable path, exit, process spawning and the clock are read only at sanctioned sites | Gated | Root `clippy.toml` (`disallowed-methods`); `#[expect]` at each sanctioned site |
| Only the shell uses the standard streams: print macros by lint, stream handles by the ban list, `dbg!` nowhere in product code | Gated | `[lints.clippy]` in the root `Cargo.toml` (`print_stdout`, `print_stderr`, `dbg_macro`); root `clippy.toml` (`std::io::stdout`, `stderr`, `stdin`); `#[expect]` at each sanctioned site |
| Every file under `src/` has exactly one module class | Gated | `crates/policy/src/classification.rs`; `crates/policy/tests/classification.rs` |
| Crate-internal imports follow the class direction | Gated | `crates/policy/tests/direction.rs`; `ALLOWED_EDGES` |
| Pure modules contain none of the effects the purity scan lists (see "Pure modules") | Gated | `crates/policy/tests/ambient.rs`; `PURE_ALLOWED_CRATES` |
| Production code declares no traits | Gated | `crates/policy/tests/traits.rs` |
| The bans still fire at every sanctioned site, and every ban path resolves | Gated | `scripts/policy-control.sh`; `crates/policy/control-expected.txt` |
| `new` and `rm` plan before they execute (ADR 0003) | Checkable | Search for `plan_forest`/`execute_plan` and `plan_rm`/`execute_rm` |
| Newtypes are validated once, at construction | Checkable | Search `src/paths.rs` for `pub fn new` on each newtype |
| When a module may become Pure | Judgment | Section "Pure modules" |
| When a trait earns its place | Judgment | Section "No production traits" |

An "ambient effect" is anything that reads or changes the outside world without being passed in: environment variables, process arguments, the working directory, the clock, the filesystem, processes, threads, the network or the standard streams. The gates detect the specific forms listed in each section below, not every possible route; "What the gates do not prove" lists the known gaps.

## Module classes

Every `.rs` file under `src/` has exactly one class, listed in `MODULES` in `crates/policy/src/classification.rs`. The classification test fails when a file is missing from that list, listed twice, or listed but gone.

- **Root**: `src/lib.rs` and the binaries in `src/bin/`. The composition root: it reads process arguments, loads configuration, dispatches commands, prints results and decides exit codes. Root may import anything.
- **Shell**: `src/version_check.rs`, a self-contained update-check subsystem with its own effects (an HTTP call, a state file, a spawned background process). Shell imports Shared and Pure.
- **Command**: every file in `src/commands/`. Commands return typed results and do not print (ADR 0002). A command imports Shared, Pure and allowlisted Command modules.
- **Shared**: `src/config.rs`, `src/forest.rs`, `src/git.rs`. Helpers used across commands. Shared imports Shared and Pure.
- **Pure**: `src/channel.rs`, `src/cli.rs`, `src/paths.rs`, `src/meta.rs` and the modules under `src/plan/`. None of the effects listed in "Pure modules" in their own source. Pure imports only Pure, plus allowlisted edges.
- **TestSupport**: `src/testutil.rs`, compiled only for tests. It is classified so that every file has a class, but it is not scanned.

Nothing except Root imports Root.

## Import direction

`crates/policy/tests/direction.rs` computes, for every module except TestSupport, the crate-internal modules it refers to, and checks each edge against the class rules above. An edge is any path that starts with `crate::`, `super::` or `self::` in the module's production code, written inline or inside a `use` tree (`use super::rm::{self, X}` counts as `super::rm`). A path is read once, from its first segment, so `super::super::channel::APP_NAME` in `commands::rm` climbs two levels and is an edge to `channel`. `super` and `self` resolve against the module's own path plus any inline `mod name { ... }` blocks around them: inside an inline module `child` of `src/channel.rs`, `super::APP_NAME` refers to `channel` itself. Each path is cut back to the longest known module, so `crate::meta::read` is an edge to `meta`. Edges to the module itself, including its inline modules, and to its own child modules are dropped. Visibility qualifiers such as `pub(crate)` and `pub(in crate::commands)` are not edges. Code inside an inline module whose attributes include `#[cfg(test)]`, whatever its visibility and other attributes, is not production code and is skipped; so are comments and string literals.

Edges the class rules refuse but that are sanctioned are listed in `ALLOWED_EDGES`, each with a reason:

- `commands::ls`, `commands::status` and `commands::reset` import `commands::branch_state`, the helper shared by the commands that inspect worktree branches. This is a permanent boundary.
- `commands::reset` imports `commands::rm`, because reset reuses rm's removal plan; whether that plan becomes a shared module is an open question (temporary).

An allowlist entry whose edge no longer exists fails the test too, so the list cannot go stale. A violation message names the edge, both classes and the rule, and says how to fix it: move the import, or add an `ALLOWED_EDGES` entry with a reason.

## Pure modules

A Pure module's production code may not contain any of the effects listed below. `crates/policy/tests/ambient.rs` scans each Pure module with a token-level scanner, so comments, strings and raw strings can neither hide nor fake a path, and aliases (`use std::env as e`), grouped imports, glob imports and raw identifiers are resolved. It reports:

- any use of `std::fs`, `std::env`, `std::process`, `std::net`, `std::time` or `std::thread`, inline or imported;
- standard input (`std::io::stdin` or `std::io::Stdin`);
- a `std::path` name with an effect: the `Path` methods that query the filesystem (`exists`, `try_exists`, `metadata`, `symlink_metadata`, `canonicalize`, `read_dir`, `read_link`, `is_file`, `is_dir` and `is_symlink`) and the function `absolute`, which reads the working directory. They are matched by name when called as a method, with or without a turbofish (`path.exists()`, `path.exists::<>()`), after `::` whether called or used as a value (`Path::exists(p)`, `.any(Path::exists)`, `std::path::absolute(p)`), and as the last segment of a `use` import (`use std::path::{absolute, Path};`). The scanner does not know types, so a method or item of another type with one of these names is reported too;
- a path into a dependency crate of the root package, meaning any key of the root `Cargo.toml`'s `[dependencies]` or `[target.*.dependencies]` tables with hyphens read as underscores, unless the crate is in `PURE_ALLOWED_CRATES` in `crates/policy/src/classification.rs`. Today that list holds `anyhow`, which carries errors; `chrono`, which supplies date and time types (its clock reads are banned by `clippy.toml`); `clap`, which `src/cli.rs` and `src/meta.rs` use to derive argument types (parsing the arguments, which reads the process's arguments, happens in `src/lib.rs`); and `serde`, which derives serialization. An allowed crate that no Pure module uses fails the test;
- `include!` and `#[path = ...]`, which pull Rust code into the module from a file the scanner does not read;
- `unsafe`, `extern`, `macro_rules!`, glob imports of `std` or `core`, importing or aliasing `std` itself, and non-ASCII identifiers.

`Path` and `PathBuf` are allowed as data; only the effectful path names above are reported. `include_str!`, `include_bytes!`, `env!` and `option_env!` are allowed: they are compile-time inputs, fixed when the binary is built, not effects at run time. (The qualified spelling `std::env!` is still refused, because the scan bans the `std::env` path.)

When may a module become Pure? When its own production code has none of the effects above and it imports only Pure modules, or holds an allowlisted edge with a reason naming how that edge goes away. `src/paths.rs` is Pure because `expand_tilde`, which reads `HOME`, lives in `src/config.rs`; likewise the metadata file's I/O (`ForestMeta::read` and `write`) lives in `src/forest.rs`, beside the Pure types in `src/meta.rs`. Reclassifying a module is a one-line change in `MODULES`; the ambient and direction tests then say whether it holds.

## No production traits

ADR 0002 keeps the call graph shallow: no traits, dependency injection or ports-and-adapters machinery. `crates/policy/tests/traits.rs` fails when production code declares a trait (`trait` followed by a name, outside test modules). Implementing a trait from another crate, such as `impl std::fmt::Display for X`, is fine.

When does a trait earn its place? Only when production code has more than one real implementation that callers must treat the same way. Test doubles are not a reason, because tests run against real git repositories (ADR 0007). Adding one is a design decision: record it in an ADR and add an entry to `ALLOWED_TRAITS` in `crates/policy/tests/traits.rs` with the reason. An entry whose trait no longer exists fails the test.

## Ambient reads

The root `clippy.toml` bans, through Clippy's `disallowed-methods`, the functions that read or change the process's surroundings: `std::env::var`, `var_os`, `vars`, `vars_os`, `args`, `args_os`, `current_dir`, `set_current_dir`, `current_exe`, `home_dir`, `set_var` and `remove_var`; `std::process::exit`, `abort` and `Command::new`; the clocks `chrono::Utc::now`, `chrono::Local::now`, `std::time::SystemTime::now` and `std::time::Instant::now`; and the `ureq` request functions `ureq::get`, `post`, `put`, `delete`, `head`, `options`, `connect`, `patch`, `trace` and `run`, with the `ureq::Agent` methods of the same names, whose one sanctioned caller is the update check in `src/version_check.rs`. Each entry carries a reason saying where the call belongs, and Clippy prints it with the error. Clippy resolves paths by definition, so aliases, glob imports and re-exports are caught. The same file bans the standard stream handles; see "Printing".

A sanctioned site carries `#[expect(clippy::disallowed_methods, reason = "...")]` on the enclosing function (or, for `src/version_check.rs`, one `#![expect(...)]` for the whole module). `#[expect]` is used instead of `#[allow]` because a stale `#[expect]` fails the build with "this lint expectation is unfulfilled": when the call goes away, the attribute must go too. Sanctioned sites are of two kinds. Permanent boundaries are where the rules say an effect belongs, and stay as long as the architecture does: the composition-root functions `main_entry`, `run` and `output` in `src/lib.rs`, the git wrapper functions `git` and `ref_exists` in `src/git.rs`, and `exec`'s spawn of the user's command; their reasons name the boundary. Temporary exceptions are effects that a planned change will move; their reasons name that change, and "Planned follow-ups" lists them. Reason text is reviewed, not checked: the build enforces that every `#[expect]` still fires, not what its reason says.

Tests are exempt. Unit tests inside `src/` get a crate-level `#![cfg_attr(test, allow(clippy::disallowed_methods, ...))]` in `src/lib.rs`; the integration tests get a crate-root `#![allow(...)]` in `tests/cli_test.rs`. Both build real git repositories and read the environment by design. Production code stays linted because Clippy also checks the library in its normal, non-test build, where the `cfg_attr` does not apply.

`clippy.toml` must stay the only one in the workspace: a per-crate `clippy.toml` replaces the root file for that crate instead of extending it.

## Printing

Only the shell uses the standard streams: commands return data and `src/lib.rs` prints it (ADR 0002). Two gates enforce this. The root package's `[lints.clippy]` table turns on `print_stdout` and `print_stderr`, and `just check` runs Clippy with `-D warnings`, so `println!`, `print!`, `eprintln!` and `eprint!` fail outside sanctioned sites; the same table turns on `dbg_macro`, so `dbg!`, which prints to standard error, fails anywhere in product code (it has no sanctioned site). The root `clippy.toml` bans `std::io::stdout`, `std::io::stderr` and `std::io::stdin`, so acquiring a stream handle to write or read through it fails too, with the reason "acquire standard streams only in src/lib.rs". Sanctioned sites carry `#[expect(clippy::print_stdout, ...)]`, `#[expect(clippy::print_stderr, ...)]` or `#[expect(clippy::disallowed_methods, ...)]`: `run`, `output`, `print_json` and `report_failure` in `src/lib.rs` (`run` also flushes standard output while printing progress, and hands standard error to `exec`'s child under `--json`), and `src/version_check.rs`. `allow-print-in-tests = true` and `allow-dbg-in-tests = true` in `clippy.toml` exempt test code from the print and `dbg!` lints; the test exemptions described in "Ambient reads" cover the stream handles.

## Positive control

A ban that silently stops firing looks exactly like a codebase with no violations. `scripts/policy-control.sh` (run by `just policy-control`, part of `just check`) proves the bans still work. It runs Clippy on the library and binaries for the stable channel with `--force-warn` on `clippy::disallowed_methods`, `clippy::print_stdout` and `clippy::print_stderr`, which makes every sanctioned site warn even though its `#[expect]` silences it in the normal build. It reduces each warning to its file and message, counts them, and compares the result with `crates/policy/control-expected.txt`; any difference fails with a diff. It also fails if Clippy reports that a `clippy.toml` path does not refer to a reachable function, so a typo in an entry whose path starts with a loaded crate, such as `std` or `chrono`, cannot disable it silently (for unloaded crates, see "What the gates do not prove").

The script forces `CARGO_TERM_COLOR=never` (CI sets `always`, and colour codes would hide every warning from the parser) and `LC_ALL=C` (so the sorted counts match on macOS and Ubuntu).

## Planned follow-ups

Temporary exceptions name the change that removes them; together they are the to-do list the build enforces. Permanent sanctioned boundaries are not on this list: the composition-root functions in `src/lib.rs`, the git wrapper in `src/git.rs`, `exec`'s spawn of the user's command, the `branch_state` helper edges and the entries in `PURE_ALLOWED_CRATES`. Reason text is reviewed, not checked.

- **Environment, home and working directory, passed in.** `xdg_dir` in `src/config.rs` reads XDG variables, `expand_tilde` in `src/config.rs` reads `HOME`, and `current_dir_preserving_symlinks` in `src/forest.rs` reads the working directory and `PWD`; `src/lib.rs` will read these and pass them in.
- **`init`'s git check goes through `src/git.rs`.** `validate_init_inputs` runs `git` directly.
- **Open question: where `version_check` belongs** in the module structure. It is classified Shell and exempted as a whole module until that is decided.
- **Open question: whether `rm`'s removal plan becomes a shared module**, which would replace the `commands::reset -> commands::rm` allowlist entry.

## Changing the rules

**Adding a module.** Create the file and add it to `MODULES` in `crates/policy/src/classification.rs` with its class. The classification test fails until you do. If the new module needs an import the class rules refuse, either restructure or add an `ALLOWED_EDGES` entry whose reason names the permanent boundary it is or the change that removes it.

**Adding an exemption.** Put `#[expect(<lint>, reason = "...")]` on the narrowest enclosing item, listing only the lints that actually fire there, with a reason that names the permanent boundary, or, for a temporary exception, the follow-up removing it. Then update `crates/policy/control-expected.txt`: the positive control fails with a diff showing the new count, which is the line to change. Add a temporary exception's follow-up to "Planned follow-ups".

**Adding a banned method.** Add an entry to `disallowed-methods` in the root `clippy.toml` with a reason saying where the call belongs. If the method has sanctioned call sites, exempt them as above; their lines in `crates/policy/control-expected.txt` prove the path resolves. If it has no call site anywhere, Clippy's reachable-function check still proves that a path under a loaded crate resolves; nothing proves a path under an unloaded crate (see "What the gates do not prove").

**Allowing a crate in Pure modules.** Add it to `PURE_ALLOWED_CRATES` in `crates/policy/src/classification.rs`, with a comment saying why the Pure module needs it and why what it uses is not an effect. An entry that no Pure module uses fails the test.

**Adding a trait or an allowed edge.** See "No production traits" and "Import direction". All three allowlists (edges, traits, crates) reject entries that are no longer used.

## What the gates do not prove

- Bare child-module paths are not followed by the direction scan: a path that starts with a child module's name, such as `init::x` or the `pub use rm::*;` re-exports in `src/commands/mod.rs`, is ignored. Only `crate::`, `super::` and `self::` paths are checked.
- Macro-generated code and text pulled in with `include!` are not scanned outside Pure modules; the scanner reads source tokens, not expanded code. In Pure modules, `macro_rules!` and `include!` are refused outright.
- The purity scan matches names, not resolved types or calls. It does not see an effect reached through an allowed crate (clap's `Parser::parse` reads the process's arguments, so a Pure module calling it would not be reported) or through trait dispatch.
- Output routes other than the print macros, the stream handles and `dbg!` are not gated: panic messages reach standard error, and Shared or Command code, where filesystem access is by design, could open `/dev/stdout` or a similar device through `std::fs`, or write to descriptors 1 and 2 through `libc` or the `unsafe` raw-descriptor APIs. The `dbg_macro` lint has no sanctioned site, so the positive control cannot show it still fires; its proof is the break-once check recorded when it was added.
- The dependency scan reads the root `Cargo.toml` line by line and takes each dependency's package key, hyphens read as underscores, as the name code uses. It does not decode TOML quoting or escapes in table names and keys (a quoted table-name segment such as `["dependencies".review_dep]`, or an escaped key such as `"review_\u0064ep"`; a plain quoted key such as `"quoted-name"` is read correctly), does not read top-level dotted keys such as `dependencies.foo = "1"`, and does not know that a dependency whose library target sets another `name` is used under that name. A dependency declared those ways escapes the crate rule in Pure modules. The guard test knows only today's ten names.
- Network requests outside `src/version_check.rs` fail through the `ureq` bans; another HTTP client added as a dependency would need its own entries. Filesystem access is reported only in Pure modules; commands read and write forests by design.
- The policy scans read only files under `src/` and the root `Cargo.toml`; integration tests, the policy crate itself and the `worker/` directory are not classified.
- A `clippy.toml` entry with no call site anywhere is covered only by Clippy's own "does not refer to a reachable function" warning, and that warning is not emitted when the path's first segment names a crate that is not loaded, so such an entry could do nothing unnoticed. This was confirmed on Clippy 0.1.95 by an experiment that replaced an unused entry with a path under an unloaded crate and saw no warning.
- The Rust toolchain is not pinned. A future stable Clippy may add lints that fail `-D warnings`, as it already could before these gates existed.
- CI installs `just` from the Ubuntu archive on an `ubuntu-24.04` runner; that step was untested when the gates were introduced, and only a push or pull request exercises it.
