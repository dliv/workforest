# CI: Deploy Cloudflare Worker from GitHub Actions

Currently `just release` deploys the worker locally via `just worker-deploy`. The `worker/wrangler.toml` is gitignored (like `.env`), so CI can't read `LATEST_VERSION` from it.

To move worker deployment into the GitHub Actions release pipeline:

1. Remove `worker/wrangler.toml` from `.gitignore` and track it (the D1 database ID and route aren't secrets)
2. Add a step to the release workflow that runs `npx wrangler deploy` in the `worker/` directory
3. Store a Cloudflare API token as a GitHub Actions secret
4. Remove the `just worker-deploy` step from `just release` (or keep it as a fallback)

This would make releases fully automated from `git push origin v0.x.y` — no local wrangler auth needed.
