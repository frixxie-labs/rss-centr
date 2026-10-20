# AGENTS.md

This repository is an RSS aggregator with a **Rust backend** (`backend/`), a shared **Rust core crate** (`core/`), a **Rust worker** (`worker/`), and a **Leptos SSR frontend in islands mode** (`frontend/`). There are no Cursor rules or Copilot instructions in this repo.

## Version Control (jj)

This repo uses Jujutsu (`jj`) for version control. Git is kept on disk for compatibility, but prefer `jj` commands.

```bash
jj status          # working-copy changes
jj diff            # current diff
jj log             # change log
jj describe -m "<message>"   # set change description
```

## Rust Commands (Cargo Workspace)

The Rust code is a workspace with `backend`, `core`, and `worker` members.

```bash
cargo check --workspace
cargo test --workspace
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo fmt --all -- --check
```

## Backend Commands (Rust / Cargo)

All commands work from the repo root with `--manifest-path backend/Cargo.toml`, or directly from `backend/`.

```bash
# Build
cargo build --manifest-path backend/Cargo.toml

# Run
cargo run --manifest-path backend/Cargo.toml

# Format (check only)
cargo fmt --manifest-path backend/Cargo.toml -- --check

# Lint (deny warnings)
cargo clippy --manifest-path backend/Cargo.toml --all-targets --all-features -- -D warnings

# Test (all)
cargo test --manifest-path backend/Cargo.toml
```

A `justfile` in `backend/` provides shortcuts: `just check`, `just lint`, `just test`, `just build`, `just prepare`.

### Running a single test

```bash
# Substring match
cargo test --manifest-path backend/Cargo.toml test_insert_feed_item

# Fully qualified path (when names collide)
cargo test --manifest-path backend/Cargo.toml feed::feed_item::tests::test_insert_feed_item

# Show stdout
cargo test --manifest-path backend/Cargo.toml test_insert_feed_item -- --nocapture

# List all tests
cargo test --manifest-path backend/Cargo.toml -- --list
```

### Environment & Database (sqlx + PostgreSQL)

`sqlx` macros (`query!`, `query_as!`) require `DATABASE_URL` at compile time. A `.env` in `backend/` sets `DATABASE_URL=postgres://postgres:postgres@localhost:5432/rss_centr`, but sqlx does **not** auto-load it. If builds fail:

```bash
export DATABASE_URL='postgres://postgres:postgres@localhost:5432/rss_centr'
```

Start the local database from the repo root before running backend commands:

```bash
docker compose up -d db
```

Migrations live in `backend/migrations/` and are auto-applied on server startup. Tests using `#[sqlx::test]` get an isolated DB per test with migrations applied.

The root `.sqlx/` directory contains cached query metadata for offline compilation (used in Docker builds with `SQLX_OFFLINE=true`). After changing SQL queries, run `cargo sqlx prepare --workspace` from the repo root to regenerate these files.

## Worker Commands (Rust / Cargo)

```bash
cargo run --manifest-path worker/Cargo.toml
cargo check --manifest-path worker/Cargo.toml
```

The worker reads `BACKEND_URL` or `--backend-url` and consumes the backend feed update queue endpoints.

### Semantic search

`GET /api/items/latest?q=...&mode=keyword|semantic|hybrid` (API default `keyword`, UI default `hybrid`). Embeddings are stored as `REAL[]` in `feed_item_embeddings` and ranked in Rust (`core/src/semantic.rs`); no Postgres extension is needed.

- Backend env: `OLLAMA_URL`, `OLLAMA_EMBED_MODEL` (default `embeddinggemma-2`), `SEMANTIC_MIN_SIMILARITY` (default `0.2`), `OLLAMA_EMBED_KEEP_ALIVE` (default `1h`; how long Ollama keeps the model loaded after a search, avoiding a ~2 s reload).
- Recency ranking (semantic/hybrid): relevance is multiplied by `(1 - w) + w * 0.5^(age / half_life)`, with the age taken from `published_at` (capped at `inserted_at`). `SEMANTIC_RECENCY_WEIGHT` (default `0.3`, `0` disables) and `SEMANTIC_RECENCY_HALF_LIFE_HOURS` (default `168`). It is applied after the similarity floor, so it only reorders relevant items.
- Worker env: `OLLAMA_URL` (default `http://desktop:11434`) is used by the indexer, which claims items via `POST /internal/embeddings/claim` and stores vectors via `PUT /internal/embeddings`. The backend decides the model.
- Run `ollama pull embeddinggemma-2` on the Ollama host. Changing `OLLAMA_EMBED_MODEL` automatically re-indexes all items.

## Frontend Commands (Leptos islands)

Run from `frontend/`. Requires `cargo-leptos` 0.3.x and the `wasm32-unknown-unknown` target.

```bash
cargo leptos watch                     # dev server on :8000 (BACKEND_URL=http://localhost:8080)
cargo leptos build --release           # ../target/release/rss-centr-frontend + ../target/site/
cargo test -p rss-centr-frontend
cargo clippy -p rss-centr-frontend --all-targets -- -D warnings
cargo clippy -p rss-centr-frontend --all-targets --target wasm32-unknown-unknown -- -D warnings
```

- Islands mode: route components in `src/pages/` and `src/app.rs` are server-only
  (compiled for non-wasm targets only) and fetch data inside `<Suspense>` with `Suspend` (routes use
  `SsrMode::Async`). Interactive UI lives in `#[island]` components in
  `src/islands/`; island props must be serializable.
- Server-rendered pages get the backend `Api` from context; islands use
  `Api::default()` (same-origin `/api/*` in the browser).
- Pass the server render time into islands and format dates in explicit UTC so
  hydration matches.
- Browser-only SSE/local storage/observers/timers live behind
  `cfg(target_arch = "wasm32")`.
- Use `try_update`/`try_set` in async tasks that may outlive an island.
- Preserve existing REST/SSE contracts. Do not expose internal worker routes.
- Tailwind is run by cargo-leptos from `style/tailwind.css`; no generated CSS is committed.
- Tests are Rust only; `server.rs` tests render every page against a mock backend.

## Code Style -- Backend (Rust)

Prefer following existing patterns over introducing new ones.

### Formatting

- Use `rustfmt` with default settings. Do not hand-format.
- Use raw strings for SQL (`r#"..."#`) with consistent indentation.

### Imports

- Group as: std, external crates, crate modules.
- Prefer explicit imports (`use anyhow::{Context, Result};`) over globs.
- Keep `use` blocks small and local to the module.

### Types and Data Modeling

- `i64` for Postgres `BIGINT`/`BIGSERIAL` primary keys.
- DB row models derive `sqlx::FromRow`; add `serde::{Serialize, Deserialize}` and `utoipa::ToSchema` for API payloads.
- `chrono::DateTime<Utc>` for timestamps.

### Naming

- Modules and functions: `snake_case` (`dequeue_due_feeds`, `insert_feed_item_detail`).
- Structs and enums: `UpperCamelCase` (`FeedItem`, `FeedItemDetail`).
- Constants: `SCREAMING_SNAKE_CASE` (`MIN_POLL_INTERVAL_SECONDS`).

### Error Handling

- Use `anyhow::Result<T>` for fallible operations.
- Add `.with_context(|| ...)` at network/IO/DB boundaries with actionable messages.
- Use `anyhow::bail!` for domain errors (e.g., "not found").
- No `unwrap()` / `expect()` outside tests.

### Async / Tokio

- Pass `reqwest::Client` as a parameter rather than constructing per-call.
- Keep async functions cancellation-safe (avoid holding locks across `.await`).

### SQLx Usage

- Prefer compile-time checked macros (`query!`, `query_as!`).
- Keep SQL close to the function that uses it; no string concatenation for SQL.
- Always bind parameters (no string interpolation into SQL).
- Check `rows_affected()` and convert zero rows to a domain error.
- Use column type annotations (`"id!: i64"`) when SQLx needs explicit Postgres type coercion.

### Handlers

- Handlers return `Result<Json<T>, HandlerError>`.
- Annotate with `#[instrument]` for tracing and `#[utoipa::path(...)]` for OpenAPI docs.
- `HandlerError` wraps HTTP status + message; see `handlers/error.rs`.

### Testing

- Unit tests go in `#[cfg(test)] mod tests { ... }` alongside the code.
- Use `#[sqlx::test]` for DB tests (isolated migrated DB per test).
- Prefer deterministic tests.
- `unwrap()` is acceptable inside tests.

## Repo Hygiene

- Do not commit secrets or `backend/.env` changes.
- Do not commit build artifacts (`*/target/`, `.leptos/`).
- If you add Cursor/Copilot rules, update this file to mirror them.
