# Command-line integration tests

Up: [Repository README](../README.md) · [Development](../docs/development.md)

[cli_test.rs](cli_test.rs) runs the built stable or beta binary. It proves
argument handling, output framing, exits, command effects, dry-runs and partial
failures. The [snapshots](snapshots/) pin reviewed output contracts; inspect
each changed transcript rather than accepting snapshot updates mechanically.

`ForestCmd` gives each process its own home and state directory and seeds an
inert version-check state. Its guard catches unexpected updater activity.
Tests that need the updater explicitly opt out. Public-network live checks and
direct background-child checks are ignored by default and state why at their
declarations. Deterministic loopback HTTP and second-host process tests run in
the ordinary update-check crate suite. Ordinary validation must not opt into
all ignored tests: that also includes the separate generated type export in
the update-check crate.

Use `just test` or `just check` for both channels. For a focused CLI test,
preserve the recipe's git isolation, for example:

```sh
GIT_CONFIG_GLOBAL=/dev/null GIT_CONFIG_NOSYSTEM=1 cargo test --locked \
  --no-default-features --features stable --test cli_test golden_usage_errors
```

The [architecture document](../docs/architecture.md#ambient-reads) owns the
test exemptions and git-isolation rationale. The ignored
`update_check_live_cli_completes` test requires the public update endpoint and
is run separately for stable and beta; it is not part of ordinary `just check`.
The deterministic loopback and second-host process proofs live with the
[update edge](../crates/update-check/README.md#proofs-and-generated-contract).
This suite does not replace the [forest's unit and second-root tests](../crates/forest/README.md#proofs)
or the [policy tests](../crates/policy/README.md). There is no worker HTTP test
suite in the Rust gate.
