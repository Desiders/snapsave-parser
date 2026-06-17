help:
    just -l

fmt:
    cargo +nightly fmt --all

# Default (rustls) features: `--all-features` would also enable
# `native-tls-vendored`, building OpenSSL from source.
lint:
    cargo clippy -- -W clippy::pedantic

test:
    cargo test

test-integration:
    cargo test -- --ignored
