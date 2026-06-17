help:
    just -l

fmt:
    cargo +nightly fmt --all

lint:
    cargo clippy --all-features -- -W clippy::pedantic
