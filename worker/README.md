# Update service worker

Up: [Repository README](../README.md) · [Development](../docs/development.md)

This Cloudflare Worker serves the protocol consumed by the Rust
[update-check edge](../crates/update-check/README.md). It is outside the Cargo
workspace and Rust architecture scans.

[src/index.ts](src/index.ts) answers `GET /api/latest`. The `channel` query
selects beta only when it equals `beta`; otherwise the stable version is
returned. Version values and the D1 binding are configured in
[wrangler.toml](wrangler.toml). The response type is generated from Rust;
follow the edge's [generation instructions](../crates/update-check/README.md#proofs-and-generated-contract)
instead of editing `src/generated/VersionResponse.ts` by hand.

The worker queues a D1 event containing city, country, supplied version,
timestamp and channel. A failed event write is logged without replacing the
version response. [schema.sql](schema.sql) defines a fresh database;
[migrations/0001_add_channel.sql](migrations/0001_add_channel.sql) adds the
channel column to the older schema, so it is not an extra step after applying
the current fresh schema.

## Operations and proof limits

The root [justfile](../justfile) owns `worker-deploy`, `worker-db-create`,
`worker-db-migrate`, `worker-logs` and `worker-query`. Deployment and database
recipes act on remote infrastructure; the database-migrate recipe executes
`schema.sql`, not the historical migration file. The
[release workflow](../.github/workflows/release.yml) also deploys the worker.

[CI](../.github/workflows/ci.yml) runs `npm ci --no-audit` and then `npm audit`
here on Node 22.
The [candidate workflow](../.github/workflows/release-candidate.yml) audits before
publication readiness; promotion audits again immediately before a required
deployment. The [release guide](../docs/releases.md) owns the separate approvals,
channel regression checks and delivery/recovery sequence. Beta publication uses
a GitHub prerelease. Preparation also runs the same audit command. These are dependency
checks, not a TypeScript typecheck or a live protocol proof.
`package.json` currently has a deploy script and no test script. The Rust
ignored network tests exercise a deployed endpoint, not this checkout's worker
implementation. Changes to worker behavior therefore need their own explicit
validation evidence.
