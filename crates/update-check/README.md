# Update-check edge

Up: [Library crates](../README.md) · [Architecture](../../docs/architecture.md)

`git-forest-update-check` is an Edge crate. It owns the update cache, the
`[version_check]` config section, notices, enablement, freshness and HTTP
protocol work. It depends on no other workspace member. This is an outbound
protocol concern, not a domain feature.

## Host contract

[lib.rs](src/lib.rs) owns the implementation and API documentation. The host
passes `Settings` with its version, app and command names, channel, endpoint,
config-file path and state-file path. It also supplies a concrete `ureq::Agent`
and a clock function to the workflows that need them. The edge does not build
an HTTP client, read a clock, or start a process.

- `check_cache_and_notify` reads config and cache state, prints the existing
  cached notice, and returns `RefreshDisposition`. `RefreshRequested` tells the
  host to start its own one-shot refresh after a successful command;
  `NoRefresh` covers disabled checks and a fresh cache.
- `force_check` performs the explicit check with the supplied agent and clock.
- `run_background_version_check` performs the refresh with those same supplied
  resources.

The binary composition root owns the HTTPS endpoint and a configured agent
with a five-second global timeout. It intercepts the private
`--internal-version-check` argument before normal CLI parsing, rebuilds the
same settings, and invokes the background entrypoint. After a successful
eligible command it resolves its own executable and starts that child with
stdin, stdout and stderr set to null, then drops the process handle without
waiting. Executable lookup and spawn failures remain best effort. The edge
keeps the HTTP request and state update; a second host can instead call the
same public workflows and choose its own process boundary.

## Proofs and generated contract

Unit tests cover cache, enablement, clock boundaries and failure behavior with
fixed inputs. [`http_contract.rs`](tests/http_contract.rs) checks the real edge
API against a bounded loopback HTTP server. [`host_contract.rs`](tests/host_contract.rs)
is a test-only second host with a real parent and child process; it proves the
parent exits before the held response is released and the child completes the
cache refresh afterward.

[CLI tests](../../tests/README.md) include an ignored live test that must fetch
the public update endpoint and prove both the built channel's cached-command
and direct-child behavior. It is run separately for stable and beta; ordinary
`just check` does not contact the public server. Other tests that contact the
server or export generated types stay ignored by default.

`VersionResponse` is the Rust owner of the response type shared with the
worker. `just generate-types` explicitly runs its ignored export test and
updates [VersionResponse.ts](../../worker/src/generated/VersionResponse.ts).
Ordinary tests do not regenerate that file. Review the generated diff when
changing the response shape.
