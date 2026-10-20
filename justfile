
lint:
    cargo clippy --workspace --all-targets --all-features -- -D warnings

frontend:
    cd frontend-dioxus && dx serve --web --port 8000

test-frontend:
    cargo test -p rss-centr-frontend --features server
    cargo check -p rss-centr-frontend --no-default-features --features web --target wasm32-unknown-unknown

upgrade:
    cargo upgrade --incompatible && cargo update
