
lint:
    cargo clippy --workspace --all-targets --all-features -- -D warnings

frontend:
    cd frontend && cargo leptos watch

test-frontend:
    cargo test -p rss-centr-frontend
    cargo clippy -p rss-centr-frontend --all-targets --target wasm32-unknown-unknown -- -D warnings

upgrade:
    cargo upgrade --incompatible && cargo update
