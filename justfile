setup:
    git config core.hooksPath .githooks

check:
    cargo fmt --all -- --check
    cargo clippy --all-targets

build:
    cargo build

test:
    cargo test

test-linux:
    docker run --rm -v "{{justfile_directory()}}:/work" -w /work rust:latest cargo test

loc:
    tokei src tests

# --- Worker (Cloudflare) ---

generate-types:
    TS_RS_EXPORT_DIR=. cargo test export_bindings -- --ignored

worker-deploy:
    cd worker && npx wrangler deploy

worker-db-create:
    cd worker && npx wrangler d1 create git-forest-version-check

worker-db-migrate:
    cd worker && npx wrangler d1 execute git-forest-version-check --remote --file=schema.sql

worker-kv-create:
    cd worker && npx wrangler kv namespace create GIT_FOREST_KV

worker-kv-seed:
    cd worker && npx wrangler kv key put --remote --binding=KV latest_version "0.2.3"

worker-logs:
    cd worker && npx wrangler tail

worker-query sql:
    cd worker && npx wrangler d1 execute git-forest-version-check --remote --command "{{sql}}"
