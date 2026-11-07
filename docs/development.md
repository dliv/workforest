# Development

Up: [Developer documentation](README.md)

Use a Rust toolchain with rustfmt and Clippy, git, `just`, and Python 3.11+ for
the full gate. Python runs the release identity validator's tests; it uses only
the standard library, including `tomllib`. Running the installed CLI does not
require Python. `tokei` is needed only for `just loc`. The Rust toolchain is not
pinned. Stable and beta are mutually exclusive Cargo features; the default
build uses stable.

## Local commands

Run these from the repository root; the [justfile](../justfile) owns their
exact commands.

| Command | Purpose |
| --- | --- |
| `just setup` | Configure this clone's git hooks |
| `just build` | Build the default stable binary |
| `cargo check --workspace --locked` | Typecheck without running the full gate |
| `just test` | Run workspace tests for both channels with global/system git configuration disabled |
| `just check` | Run formatting, Clippy, tests for both channels, release-identity fixtures, and the policy positive control |
| `just release-tests` | Test release tag/version/commit and worker-channel identity in disposable Git fixtures |
| `just policy-control` | Check that sanctioned Clippy sites still produce the expected warnings |
| `just loc` | Count source lines |
| `just test-linux` | Run the existing Docker test recipe; it is not a substitute for `just check` |

Run `just check` before committing or handing work back. [CI](../.github/workflows/ci.yml)
runs the same gate, plus a separate worker dependency audit. The
[architecture document](architecture.md) owns the exact enforcement claims
and limits; [tests/README.md](../tests/README.md) maps the proofs and explains
how to preserve test isolation for a focused Cargo invocation. Ignored tests
are not part of the ordinary gate.

## Releases and the worker

Follow the commit instructions in [AGENTS.md](../AGENTS.md). The macOS-only
`just release <version>` and `just release-beta <version>` recipes validate,
bump the channel's version, commit, tag and push. They are publishing actions,
not local build checks. The [release validator](../scripts/README.md#release-preflight)
checks clean exact-tag/Cargo/worker identity. Tag CI runs that check and the full
gate, then builds and inspects the matching
binary on each supported native macOS architecture, deploys the worker
and updates the Homebrew tap; [release.yml](../.github/workflows/release.yml)
owns that sequence. When pushing a tag manually, name the tag rather than
using `--tags`.

The [worker README](../worker/README.md) owns its source map, generated
response type and remote-operation recipes. Do not run release or worker
deployment recipes merely to validate a documentation change.
