# Update-check edge

Up: [Library crates](../README.md) · [Architecture](../../docs/architecture.md)

`git-forest-update-check` is an Edge crate. It talks to the
[update worker](../../worker/README.md), caches the last check, and reports
available versions. It depends on no other workspace member. This is an
outbound protocol concern, not a domain feature.

## Current host contract

[lib.rs](src/lib.rs) owns the implementation and API documentation. The host
passes `Settings`: its version, app/command names, channel, state-file path
and config-file path. The edge reads only its own `[version_check]` table.

- `check_cache_and_notify` handles the cached notice and may launch a refresh.
- `force_check` performs the explicit check and returns a `ForceCheckResult`.
- `run_background_version_check` performs the detached refresh.

Today the edge builds its HTTP client, reads the clock, prints cached notices
and spawns the host's executable with `INTERNAL_VERSION_CHECK_ARG`. That child
receives no settings: the host must intercept the argument before normal CLI
parsing, rebuild the same settings and call the background entrypoint. A
custom foreground state path does not automatically reach the child. A
second host must honor this handshake or supply its own spawning boundary.

The agreed move to app-owned configured resources and background lifetime is
[planned follow-up work](../../docs/architecture.md#planned-follow-ups).
The API above describes the implementation that exists now.

## Proofs and generated contract

Unit tests in `lib.rs` cover version comparison, state, enablement and disabled
checks. [CLI tests](../../tests/README.md) cover the assembled host; tests that
contact the update server or start the background check are ignored by default.
The forest's second-root test does not cover this handshake.

`VersionResponse` is the Rust owner of the response type shared with the worker.
`just generate-types` explicitly runs its ignored export test and updates
[VersionResponse.ts](../../worker/src/generated/VersionResponse.ts). Ordinary
tests do not regenerate that file. Review the generated diff when changing
the response shape.
