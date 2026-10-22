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
