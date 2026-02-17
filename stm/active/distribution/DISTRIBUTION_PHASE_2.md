# Phase 2 — Cloudflare Infrastructure

Standing up the backend for version check at `forest.dliv.gg`.

Prerequisite: Phase 1 code changes are merged and the client-side version check is in place (gracefully failing until this phase completes).

## 1. DNS: `forest.dliv.gg` Subdomain

In Cloudflare dashboard for `dliv.gg`:

- **Do not** add a DNS record manually for `forest` — Cloudflare Workers Custom Domains handle this automatically
- Instead, use Workers > your worker > Settings > Domains & Routes > Add Custom Domain: `forest.dliv.gg`
- Cloudflare creates the necessary DNS record and SSL cert automatically
- Existing records for `dliv.gg`, `lil-yot-0-local`, `ly3`, `www` are untouched

## 2. Create D1 Database

```bash
wrangler d1 create git-forest-version-check
```

Note the database ID from the output.

### Schema

```sql
CREATE TABLE events (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  ip TEXT NOT NULL,
  version TEXT NOT NULL,
  timestamp TEXT NOT NULL
);

CREATE INDEX idx_events_timestamp ON events(timestamp);
CREATE INDEX idx_events_ip ON events(ip);
```

Apply:
```bash
wrangler d1 execute git-forest-version-check --file=schema.sql
```

## 3. Create KV Namespace

```bash
wrangler kv namespace create GIT_FOREST_KV
```

Note the namespace ID from the output.

Seed the initial version:
```bash
wrangler kv key put --namespace-id=<id> latest_version "0.1.0"
```

## 4. Worker Code

Create a directory for the worker (separate from the git-forest repo, or in a subdirectory — your call):

### `worker.js`

```javascript
export default {
  async fetch(request, env) {
    const url = new URL(request.url);

    if (url.pathname !== "/api/latest") {
      return new Response("Not found", { status: 404 });
    }

    if (request.method !== "GET") {
      return new Response("Method not allowed", { status: 405 });
    }

    const version = url.searchParams.get("v") || "unknown";
    const ip = request.headers.get("cf-connecting-ip") || "unknown";
    const timestamp = new Date().toISOString();

    // Log to D1 (best-effort, don't fail the response)
    try {
      await env.DB.prepare(
        "INSERT INTO events (ip, version, timestamp) VALUES (?, ?, ?)"
      )
        .bind(ip, version, timestamp)
        .run();
    } catch (e) {
      console.error("D1 write failed:", e);
    }

    // Return latest version from KV
    const latest = await env.KV.get("latest_version");
    return Response.json({
      version: latest || "0.1.0",
    });
  },
};
```

### `wrangler.toml`

```toml
name = "git-forest-api"
main = "worker.js"
compatibility_date = "2026-02-16"

[[d1_databases]]
binding = "DB"
database_name = "git-forest-version-check"
database_id = "<from step 2>"

[[kv_namespaces]]
binding = "KV"
id = "<from step 3>"
```

## 5. Deploy Worker

```bash
wrangler deploy
```

Then add the custom domain in the Cloudflare dashboard:
- Workers & Pages > `git-forest-api` > Settings > Domains & Routes > Custom Domains > Add `forest.dliv.gg`

### Verify

```bash
curl https://forest.dliv.gg/api/latest?v=0.1.0
# Expected: {"version":"0.1.0"}
```

## 6. Wire Release Workflow to Update KV

Add an `update-version` job to `.github/workflows/release.yml`:

```yaml
  update-version:
    needs: build
    runs-on: ubuntu-latest
    steps:
      - name: Update latest version in Cloudflare KV
        env:
          CF_API_TOKEN: ${{ secrets.CF_API_TOKEN }}
          CF_ACCOUNT_ID: ${{ secrets.CF_ACCOUNT_ID }}
          CF_KV_NAMESPACE_ID: ${{ secrets.CF_KV_NAMESPACE_ID }}
        run: |
          VERSION="${GITHUB_REF_NAME#v}"
          curl -X PUT \
            "https://api.cloudflare.com/client/v4/accounts/$CF_ACCOUNT_ID/storage/kv/namespaces/$CF_KV_NAMESPACE_ID/values/latest_version" \
            -H "Authorization: Bearer $CF_API_TOKEN" \
            -H "Content-Type: text/plain" \
            --data "$VERSION"
```

### GitHub Secrets to Add

- `CF_API_TOKEN` — Cloudflare API token with KV write permission
- `CF_ACCOUNT_ID` — Cloudflare account ID
- `CF_KV_NAMESPACE_ID` — KV namespace ID from step 3

## 7. Querying Usage Data

From the Cloudflare dashboard or via `wrangler d1 execute`:

```sql
-- Active users (unique IPs in last 7 days)
SELECT ip, version, COUNT(*) as hits, MAX(timestamp) as last_seen
FROM events
WHERE timestamp > datetime('now', '-7 days')
GROUP BY ip
ORDER BY last_seen DESC;

-- Version adoption
SELECT version, COUNT(DISTINCT ip) as users
FROM events
WHERE timestamp > datetime('now', '-30 days')
GROUP BY version;
```

## 8. End-to-End Test

After deploying:

1. `curl https://forest.dliv.gg/api/latest?v=0.1.0` — should return `{"version":"0.1.0"}`
2. `git forest version --check` — should show current version and latest (matching, since both are 0.1.0)
3. Manually update KV to a fake newer version:
   ```bash
   wrangler kv key put --namespace-id=<id> latest_version "99.0.0"
   ```
4. Wait for daily cache to expire (or delete state.toml), run any command — should see update notice on stderr
5. Reset KV back to actual version

## 9. Future Improvements (Not in Scope)

- Automate Homebrew formula updates in the release workflow (push SHA256 updates to `dliv/homebrew-tools`)
- Add Linux builds to the release matrix
- Retention policy for D1 events (delete old rows periodically)
