# 16. Gated Architecture Rules

Date: 2026-10-12
Status: Accepted

## Context

The architecture rules in these decision records were enforced only by review. ADR 0002 says commands return data and do not print, configuration and the environment are read at the edge, and there are no traits; nothing failed when code broke those rules, and some code already had (`src/commands/exec.rs` prints from inside a command). A violation was found after a push, if at all.

The rules that can be checked mechanically are cheap to check: Clippy can ban specific functions and printing, and a small test-only crate can read the sources and check module structure. A gate that fails locally in about a minute moves the feedback to the person or agent writing the code.

## Decision

`just check` is the one gate, and CI runs exactly that command. It runs formatting, Clippy for both build channels across the whole workspace (every Cargo command passes `--workspace`, so no member is skipped, and `--locked`), all tests for both channels, and a positive control.

- A root `clippy.toml` bans ambient reads (environment, process arguments, working directory, executable path, exit, process spawning, clocks) through `disallowed-methods`, each with a reason naming where the call belongs. The root package's `[lints.clippy]` table turns on `print_stdout` and `print_stderr`.
- Every sanctioned site carries `#[expect(..., reason = "...")]`, never `#[allow]`, so a stale exemption fails the build. Each reason names the follow-up that removes it.
- A workspace member, `crates/policy` (`git-forest-policy`, test-only, no dependencies), classifies every file under `src/` and tests that imports follow the class direction, that Pure modules contain no ambient effects, and that production code declares no traits. Its scanner works on tokens, so comments and strings cannot hide or fake a path.
- `scripts/policy-control.sh` forces the banned lints on and compares the warnings at every sanctioned site with `crates/policy/control-expected.txt`, so a ban that stops firing, or a ban path that resolves to nothing, fails the gate.

The rules, their kinds (gated, checkable by search, or judgment) and their homes are documented in `docs/architecture.md`.

## Consequences

- A violation of a gated rule fails `just check` locally and in CI, with a message naming the rule and the `docs/architecture.md` section that explains it.
- The exemptions are a visible to-do list the build enforces; `docs/architecture.md` lists them under "Planned follow-ups".
- The repository is a Cargo workspace whose root is still the `git-forest` package. Release builds name the binary target and never compile the policy crate; `just release` still bumps only the `[package]` version.
- `just check` now includes the full test suite, so `just release`, which runs `just check` and then `just test`, runs the tests twice.
- Adding a module, an exemption, an allowed edge, a trait or a banned method is a deliberate, reviewed change to policy data (see "Changing the rules" in `docs/architecture.md`).
- The toolchain is not pinned; a new stable Clippy can still add lints that fail `-D warnings`.

Revisit if: the policy engine moves into a shared crate used by other projects, or a rule's exemptions come to outnumber its uses.
