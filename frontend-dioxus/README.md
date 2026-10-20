# RSS Centr — Dioxus frontend

Rust/Dioxus **0.7**, with server-rendered initial data and browser hydration.
The existing backend/worker APIs and database are unchanged. The previous Fresh
frontend remains in `../frontend/` during the reversible deployment cutover.

## Development

Requirements: stable Rust, the `wasm32-unknown-unknown` target and a Dioxus CLI
0.7.x matching the `dioxus` crate. No Deno or Node is needed.

```sh
rustup target add wasm32-unknown-unknown
cargo install dioxus-cli --version ^0.7 --locked   # or: cargo binstall dioxus-cli@^0.7
export PATH="$HOME/.cargo/bin:$PATH"
BACKEND_URL=http://localhost:8080 dx serve --web --port 8000
```

`dx` runs Tailwind itself (it downloads the standalone CLI), using
`tailwind_input`/`tailwind_output` in `Dioxus.toml`, and rebuilds
`assets/styles.css` on `dx serve` and `dx build`. The generated file is
committed so plain `cargo` builds and tests work; commit it after changing
utility classes.

**Deno also provides a command named `dx`.** Check `dx --version`: it should say
`dioxus 0.7.x`, not print the Deno version.

## Build and test

From this directory:

```sh
cargo test -p rss-centr-frontend --features server
cargo fmt --all -- --check
cargo clippy -p rss-centr-frontend --all-targets --all-features -- -D warnings
cargo check -p rss-centr-frontend --no-default-features --features web --target wasm32-unknown-unknown
dx build --web --release --profile wasm-release --package rss-centr-frontend --no-default-features --locked
```

Browser (SSR/hydration) integration tests were removed together with Deno and
Playwright; porting them to Rust is pending.

## Production

From the repository root:

```sh
docker build -f frontend-dioxus/Dockerfile -t rss-centr-frontend:dioxus .
docker compose up --build frontend
```

For a local production build, run `server` from
`target/dx/rss-centr-frontend/release/web/` alongside its `public/` directory:

```sh
IP=127.0.0.1 PORT=8000 BACKEND_URL=http://localhost:8080 ./server
```

- `IP` / `PORT`: listening address; container defaults are `0.0.0.0:8000`.
- `BACKEND_URL`: server-only backend base URL, default `http://localhost:8080`.
- `RUST_LOG`: structured tracing filter, default `info`.
- `/api/*`: same-origin REST and unbuffered SSE proxy. Internal worker endpoints
  are not exposed. Configure upstream reverse proxies not to buffer SSE.
- `/status/ping`: frontend liveness.
- `/health` and `/status/health`: backend-aware readiness (`200` or `503`).

## Compatibility and intentional differences

- `/`, `/timeline`, `/news`, `/sources`, `/topics` all render initial backend
  data into HTML. Dioxus serializes SSR futures for hydration rather than
  refetching.
- Legacy redirects preserve query strings and return `308`.
- News retains hybrid defaults, 250 ms debounce, cancellation/stale-result
  protection and backend relevance ordering for semantic/hybrid queries.
- Timeline uses native `EventSource`, persisted resume IDs, batched replay,
  deduplication, a 500-item cap, detail enrichment and scoped cleanup.
- RSS HTML is reduced to escaped plain-text previews, never injected as markup.
- Dates use explicit **UTC** on server and browser to avoid timezone-dependent
  hydration mismatches (the previous UI used locale-dependent dates).
- Source operations are serialized while pending; deletion uses an inline
  confirmation instead of `window.confirm`.

## Rollback

Redeploy the previous Fresh container image under the same frontend service,
port and `BACKEND_URL`. No API or database migration is needed. The Fresh source
is retained until deployment acceptance; removing it is a separate cleanup.
