# 1. Agent-Drivable First

Date: 2026-02-15
Status: Accepted

## Context

git-forest's primary consumer is a software agent — MCP tools, AI coding assistants, shell scripts. Human UX matters but is secondary. This constraint must be baked into the CLI design from the start, not retrofitted, because it affects input handling, output format, error reporting, and exit codes across every command.

## Decision

Design the CLI so that every operation is fully automatable without interactive prompts:

- **All inputs as flags.** No command requires interactive prompts to function. Every parameter is a clap flag or argument (`src/cli.rs`). Interactive features (Phase 7) only activate when stdin is a TTY and required flags are missing.
- **`--json` on every command.** A global `--json` flag (`src/cli.rs`) switches all output to machine-readable JSON. Both human and JSON formats are backed by the same result structs — `NewResult`, `RmResult`, `LsResult`, `StatusResult`, `ExecResult` — all deriving `Serialize`. A command-level failure under `--json` prints one document on stdout, `{"error": {"code": ..., "message": ..., "hint": ...}}`, with the code `usage` or `command-failed`, `hint` null when there is none, and nothing on stderr. `update` is the exception: it streams Homebrew's own output, which is not JSON, so `--json update` is refused as a usage error (exit 2) before it runs or prints anything else.
- **Actionable error messages.** Errors include a hint where a recovery step exists (e.g., `"hint: run git forest init --repo <path>"`), parseable by agents.
- **Predictable exit codes.** A closed set: 0 success; 1 any failure, including an incomplete result, a dry-run that predicts failure, and `reset` without `--confirm`; 2 usage errors, meaning clap's errors and `run`'s own flag-format errors (a malformed `repo=branch` value, a missing `--feature-branch-template`). A wrong value, such as an unknown repo name, is 1. `main_entry` in `src/lib.rs` maps every outcome to its exit code in one place. A panic's exit code 101 is not part of the interface.
- **`--dry-run` for inspection.** Agents can review plans via `--json --dry-run` before approving execution.

## Consequences

- No interactive-only features exist; the wizard (Phase 7) is a convenience layer, not the core interface.
- `--json` output is always available and structurally identical to human output data.
- Error messages are verbose by design — they carry hints where a recovery step exists, not just status codes.
- An agent reads one JSON document on stdout under `--json` whether the command succeeds or fails, and tells usage errors (exit 2) from other failures (exit 1) without parsing text.
- This decision drives ADR 0002 (commands return data, enabling `--json`) and ADR 0003 (`--dry-run` via plan/execute).

Revisit if: a command needs input that cannot be given as a flag, or agents come to drive the tool through another interface, such as a library or an MCP server.
