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
