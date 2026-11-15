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

## Release operations

[The release guide](../docs/releases.md) owns the operator sequence and approval
boundaries. [release.py](release.py) is the Python standard-library operations
entrypoint; it uses subprocess argument arrays, the authenticated `gh` CLI and
ordinary Git. It does not infer human consent. Run it through the named `just`
recipes. Its internal `manifest` command binds candidate artifact IDs/digests;
`promote` is the trusted main-dispatch publication entrypoint.

[check_release.py](check_release.py) validates clean source, root Cargo/lockfile
identity and the selected worker version. `--candidate VERSION --channel CHANNEL
--notes` works before a tag exists, even for the bootstrap's existing version.
The retained positional `vVERSION` invocation additionally requires that exact
lightweight or annotated tag to resolve to HEAD. Supported versions are stable
`X.Y.Z` and beta `X.Y.Z-beta.N` with no leading zeroes. Only successful validation
emits constrained version/channel/binary outputs.

`just release-tests` (also in `just check`) runs real temporary Git merge and
Cargo preparation fixtures, metadata/artifact/digest refusal tests and promotion
recovery tests. API/worker/upload fault-injection tests use explicit substitutes;
they prove guards and ordering, not hosted delivery. Native package checks live
in [release-candidate.yml](../.github/workflows/release-candidate.yml). The
publication workflow contains no Cargo build and promotes those exact archives.
