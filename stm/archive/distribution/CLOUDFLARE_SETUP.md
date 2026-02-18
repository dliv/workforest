# Cloudflare Setup Checklist ✅

All steps completed 2026-02-17.

## 0. Prerequisites

- [x] `npx wrangler login` (interactive OAuth, one-time)
- [x] `cd worker && npm install` (get wrangler + workers-types locally)

## 1. Create D1 database

- [x] `just worker-db-create`
- [x] Copy `database_id` into `worker/wrangler.toml` → `28dbcad0-a42c-4a7d-bb9a-aebf335adff5`

## 2. Apply schema

- [x] `just worker-db-migrate` (with `--remote` — fixed in justfile)

## 3. Create KV namespace

- [x] `just worker-kv-create`
- [x] Copy namespace `id` into `worker/wrangler.toml` → `12a2af8dcfaa43b0a433cc1e8c459ec9`

## 4. Seed KV with current version

- [x] `just worker-kv-seed` (with `--remote` — fixed in justfile)

## 5. Deploy the worker

- [x] `just worker-deploy`
- [x] Custom domain configured via `wrangler.toml` (`custom_domain = true`), no dashboard needed

## 6. Smoke test

- [x] `curl "https://forest.dliv.gg/api/latest?v=0.2.3"` → `{"version":"0.2.3"}`
- [x] `git forest version --check` → hit logged to D1
- [x] `just worker-query "SELECT * FROM events ORDER BY id DESC LIMIT 5"` → row from Oxford, US

## 7. GitHub Secrets (for release workflow)

Values also in `worker/.env` (gitignored).

- [x] `CLOUDFLARE_API_TOKEN` — Cloudflare API token with KV write permission
- [x] `CLOUDFLARE_ACCOUNT_ID`
- [x] `CF_KV_NAMESPACE_ID`
- [x] `HOMEBREW_TAP_TOKEN` — fine-grained PAT with contents write on `dliv/homebrew-tools`
- [x] All four added to GitHub: repo Settings > Secrets and variables > Actions (repo secrets)

## 8. Back up secrets

- [x] Fill in `worker/.env` with the three values (local reference, gitignored)
- [x] Back up to Bitwarden
