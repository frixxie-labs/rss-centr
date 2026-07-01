
lint:
    cargo clippy --workspace --all-targets --all-features -- -D warnings
    cd frontend && deno task check

upgrade:
    cargo upgrade --incompatible && cargo update
    cd frontend && deno task update && deno update
