# Repository scripts

Up: [Repository README](../README.md) · [Development](../docs/development.md)

[policy-control.sh](policy-control.sh) is the architecture positive control,
invoked by `just policy-control` and `just check`. It runs Clippy with forced
warnings, writes `target/policy-control.log`, and compares per-file/message
counts with [control-expected.txt](../crates/policy/control-expected.txt).
It also rejects unresolved ban paths when Clippy reports them.

[Architecture: Positive control](../docs/architecture.md#positive-control)
owns the exact claim and limits. A matching count is not proof of which
function owns a call or how often it executes. The
[policy README](../crates/policy/README.md) maps the other checks.

## Release preflight

`python3 scripts/check_release.py v<version>` (Python 3.11+) reads clean tagged
source, requires the tag's commit to be HEAD, and matches root Cargo version
to the selected `LATEST_VERSION_STABLE` or `LATEST_VERSION_BETA` worker variable.
Supported channels are stable `X.Y.Z` and beta `X.Y.Z-beta.N`. The script accepts
existing lightweight and annotated tags. It emits constrained channel/binary
outputs only after all checks pass and performs no publication or deployment.

`just release-tests`, also in `just check`, uses disposable real Git repositories
to exercise accepted and rejected identities. CI installs Python explicitly.
The release workflow runs the full gate before locked native macOS builds;
architecture, executable version and exact archive bytes are checked before upload.
