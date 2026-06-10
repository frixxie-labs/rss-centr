
upgrade:
    cd backend && cargo upgrade --incompatible && cargo update
    cd frontend && deno update
