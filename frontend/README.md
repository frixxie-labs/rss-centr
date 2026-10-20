# RSS Centr — Leptos frontend

Rust/Leptos **0.8** in **islands mode**. Pages are server-rendered with data from
the backend; only interactive parts (`#[island]` components in `src/islands/`)
ship WASM and hydrate. Navigation between pages is plain full-page loads.

## Layout

- `src/app.rs`, `src/pages/` — server-only shell, router and route components
  (compiled for non-wasm targets only). They fetch data and pass it to islands as serialized props.
- `src/islands/` — interactive islands (summary controls, timeline, news,
  sources, topics). Compiled into both the server and the WASM bundle.
- `src/components.rs` — shared components used inside islands.
- `src/browser.rs` — browser-only SSE, IntersectionObserver and localStorage.
- `src/server.rs` — Axum router: Leptos routes, `/api/*` proxy (incl. SSE),
  health endpoints and legacy redirects.

## Development

Requirements: stable Rust, the `wasm32-unknown-unknown` target and
`cargo-leptos` 0.3.x. Tailwind is run by cargo-leptos (`tailwind-input-file`).

```sh
rustup target add wasm32-unknown-unknown
cargo install cargo-leptos --locked
BACKEND_URL=http://localhost:8080 cargo leptos watch   # http://127.0.0.1:8000
```

## Build and test

```sh
cargo test -p rss-centr-frontend
cargo clippy -p rss-centr-frontend --all-targets -- -D warnings
cargo clippy -p rss-centr-frontend --all-targets --target wasm32-unknown-unknown -- -D warnings
cargo leptos build --release
```

Output: `../target/release/rss-centr-frontend` and `../target/site/`.

## Production

```sh
docker build -f frontend/Dockerfile -t rss-centr-frontend .
```

To run a local release build:

```sh
LEPTOS_SITE_ROOT=target/site IP=127.0.0.1 PORT=8000 BACKEND_URL=http://localhost:8080 \
  target/release/rss-centr-frontend
```

- `IP` / `PORT`: listening address (falls back to `LEPTOS_SITE_ADDR`).
- `BACKEND_URL`: server-only backend base URL, default `http://localhost:8080`.
- `LEPTOS_SITE_ROOT`: directory containing `pkg/` and static assets.
