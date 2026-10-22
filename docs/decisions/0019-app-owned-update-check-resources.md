# 19. App-Owned Update-Check Resources and Process Lifetime

Date: 2026-10-22
Status: Accepted and implemented

## Context

The update-check edge formerly created its own HTTP client, read the clock,
printed cached notices and relaunched the host executable for a background
check. That worked for git-forest but made the library's resource policy and
process lifetime implicit in the edge. A second host either had to reproduce
the hidden executable handshake or could not use the same cache workflow.

The change is about ownership. The observable CLI contract, update protocol,
cache schema and best-effort failure behavior are already established and stay
in place. The update checker remains an edge client, not a new feature.

## Decision

The application composition root owns the concrete configured HTTP agent,
endpoint and clock, and it owns the private child-process argument and the
one-shot detached launch. The update-check edge receives those resources and
continues to own config enablement, cache reads and writes, freshness,
protocol calls, and existing notices. Its cached-check API returns a typed
`RefreshDisposition`; `RefreshRequested` asks the host to start a child after
the successful command completes, while `NoRefresh` covers disabled checks
and fresh cache state.

The production host uses a five-second global HTTP timeout. It launches the
same executable with null standard streams, does not wait for the child, and
treats executable lookup and spawn failures as best effort. The child rebuilds
the same settings from the host's resolved environment paths before invoking
the edge refresh function. A second host may instead call the public edge
workflows directly and own a different process boundary.

No state format, cache concurrency policy, runtime, queue, daemon, async task
model or foreground command behavior is introduced. Persisted timestamp
semantics remain intentional: background refresh keeps `last_checked` from a
readable state and initializes it from the supplied clock when state is
unavailable; freshness is evaluated after loading cache state; and an explicit
check persists its completion-time sample after the HTTP result.

## Feature-start answers

1. **Behavior:** No command output, notice, exit code, request shape, cache
   schema, or failure result changes. A successful eligible command still
   requests a detached refresh for missing, stale or incomplete cache state;
   disabled checks and fresh cache state do not. The child remains silent.
2. **Placement and invariants:** This extends the existing update-check edge
   and binary composition root. The edge owns cache and protocol decisions;
   the application owns configured resources and executable lifetime. Preserve
   timestamp sampling order, the five-second request timeout, one-shot
   detachment and unchanged state-file contents.
3. **Trust and effects:** The host supplies the endpoint, application identity,
   channel, file paths, agent and clock. The edge reads only its
   `[version_check]` configuration table and treats an absent or malformed
   value as enabled, as before. Network access and process creation remain at
   explicit boundaries.
4. **Failure and lifecycle:** There is no queue, deduplication, cancellation
   protocol or cache lock. Lookup, spawn, fetch and persistence failures remain
   best effort. The foreground never waits for the refresh child; the child
   uses the same configured agent timeout. Concurrent cache writes retain the
   existing behavior and are not made atomic by this change.
5. **Proof:** Deterministic edge tests inject agents and clocks; loopback HTTP
   tests exercise request/response and failure behavior; a second host uses a
   real test parent and child process to prove detachment and later cache
   completion. Ignored live CLI tests for both channels prove the assembled
   binary can refresh its real temporary XDG cache. The ordinary gate proves
   compilation, policy and offline tests, not public-server availability or
   exact runtime ownership by itself.
6. **Architecture change:** This applies the accepted app-owned resource and
   lifecycle rule without changing the crate graph or effectful call graph for
   cache/protocol work. Narrow Clippy and policy updates record the new root
   sites. No generalized injection framework or runtime is added.

## Consequences

- A consumer can choose the HTTP client and clock policy without replacing
  cache, enablement or protocol behavior.
- The application decides whether and how to start a one-shot process. The
  update edge has no dependency on the host's executable or subprocess
  convention.
- Root policy now sanctions `std::env::current_exe` only for the app-owned
  refresh launcher; the Pure profile continues to forbid it. Clippy still
  requires a narrow `#[expect]` at the call site. The positive control counts
  the new root lookup and process launch.
- The root package declares `ureq` to construct its concrete `Agent`, while
  every request method remains banned outside the update-check edge.
- `http_contract.rs` and `host_contract.rs` use bounded local fixtures and
  test-owned processes. The ignored `update_check_live_cli_completes` test
  separately requires public network access and is run for stable and beta.
- No worker contract or generated binding changes.

Revisit if a second production host, a request-policy change, or a background
service changes the resource or lifecycle guarantees. Cache concurrency and
crash-safe state replacement remain separate decisions.
