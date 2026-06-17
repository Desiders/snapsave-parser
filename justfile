help:
    just -l

fmt:
    cargo +nightly fmt --all

lint:
    cargo clippy --all-features -- -W clippy::pedantic

test:
    cargo test

test-integration:
    cargo test -- --ignored
