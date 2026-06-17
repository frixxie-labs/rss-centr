
lint:
    cd backend && cargo clippy --all-targets --all-features -- -D warnings
    cd frontend && deno task check

upgrade:
    cd backend && cargo upgrade --incompatible && cargo update
    cd frontend && deno task update && deno update
