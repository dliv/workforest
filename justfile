setup:
    git config core.hooksPath .githooks

check:
    cargo fmt --all -- --check
    cargo clippy --all-targets

build:
    cargo build

test:
    cargo test

loc:
    tokei src tests
