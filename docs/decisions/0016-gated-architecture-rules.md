# 16. Gated Architecture Rules

Date: 2026-10-12
Status: Accepted (updated after the first independent review of the gates)

## Context

The architecture rules in these decision records were enforced only by review. ADR 0002 says commands return data and do not print, configuration and the environment are read at the edge, and there are no traits; nothing failed when code broke those rules, and some code already had (`src/commands/exec.rs` prints from inside a command). A violation was found after a push, if at all.

The rules that can be checked mechanically are cheap to check: Clippy can ban specific functions and printing, and a small test-only crate can read the sources and check module structure. A gate that fails locally in about a minute moves the feedback to the person or agent writing the code.

## Decision

`just check` is the one gate, and CI runs exactly that command. It runs formatting, Clippy for both build channels across the whole workspace (every Cargo command passes `--workspace`, so no member is skipped, and `--locked`), all tests for both channels, and a positive control.

- A root `clippy.toml` bans ambient reads (environment, process arguments, working directory, executable path, exit, process spawning, clocks) and the standard stream handles through `disallowed-methods`, each with a reason naming where the call belongs. The root package's `[lints.clippy]` table turns on `print_stdout`, `print_stderr` and `dbg_macro`.
- Every sanctioned site carries `#[expect(..., reason = "...")]`, never `#[allow]`, so a stale exemption fails the build. A sanctioned site is either a permanent boundary (the composition root in `src/lib.rs`, the git wrapper in `src/git.rs`, `exec`'s spawn of the user's command), whose reason names the boundary, or a temporary exception, whose reason names the follow-up that removes it. Reason text is reviewed, not checked.
- A workspace member, `crates/policy` (`git-forest-policy`, test-only, no dependencies), classifies every file under `src/` and tests that imports follow the class direction, that Pure modules contain none of the listed effects (banned `std` modules, standard input, effectful `std::path` names, dependency crates other than an allowlist, environment variables Cargo does not set from the manifest, data includes from outside the repository), and that production code declares no traits. Its scanner works on tokens, so comments and strings cannot hide or fake a path.
- `scripts/policy-control.sh` forces the banned lints on and compares the warnings at every sanctioned site with `crates/policy/control-expected.txt`, so a ban that stops firing, or a ban path under a loaded crate that resolves to nothing, fails the gate (paths under unloaded crates are a documented limit in `docs/architecture.md`).

The rules, their kinds (gated, checkable by search, or judgment) and their homes are documented in `docs/architecture.md`.

## Consequences

- A violation of a gated rule fails `just check` locally and in CI, with a message naming the rule and the `docs/architecture.md` section that explains it.
- The temporary exceptions are a visible to-do list the build enforces; `docs/architecture.md` lists them under "Planned follow-ups", apart from the permanent boundaries.
- The purity scan matches names, not resolved types: effects reached through an allowed crate, trait dispatch or an allowlisted import are not seen, and a same-named method or item of another type is reported; `docs/architecture.md` lists these limits under "What the gates do not prove".
- The repository is a Cargo workspace whose root is still the `git-forest` package. Release builds name the binary target and never compile the policy crate; `just release` still bumps only the `[package]` version.
- `just check` now includes the full test suite, so `just release`, which runs `just check` and then `just test`, runs the tests twice.
- Adding a module, an exemption, an allowed edge, a trait, an allowed crate or a banned method is a deliberate, reviewed change to policy data (see "Changing the rules" in `docs/architecture.md`).
- The toolchain is not pinned; a new stable Clippy can still add lints that fail `-D warnings`.

Revisit if: the policy engine moves into a shared crate used by other projects, or a rule's exemptions come to outnumber its uses.
