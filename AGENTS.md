# AGENTS.md

This repository is an RSS aggregator with a **Rust backend** (`backend/`) and a **Deno Fresh 2 frontend** (`frontend/`). There are no Cursor rules or Copilot instructions in this repo.

## Version Control (jj)

This repo uses Jujutsu (`jj`) for version control. Git is kept on disk for compatibility, but prefer `jj` commands.

```bash
jj status          # working-copy changes
jj diff            # current diff
jj log             # change log
jj describe -m "<message>"   # set change description
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

### Environment & Database (sqlx + SQLite)

`sqlx` macros (`query!`, `query_as!`) require `DATABASE_URL` at compile time. A `.env` in `backend/` sets `DATABASE_URL=sqlite:dev.db`, but sqlx does **not** auto-load it. If builds fail:

```bash
export DATABASE_URL='sqlite:dev.db'
```

Migrations live in `backend/migrations/` and are auto-applied on server startup. Tests using `#[sqlx::test]` get an isolated DB per test with migrations applied.

The `backend/.sqlx/` directory contains cached query metadata for offline compilation (used in Docker builds with `SQLX_OFFLINE=true`). After changing SQL queries, run `cargo sqlx prepare --workspace` from `backend/` to regenerate these files.

## Frontend Commands (Deno / Fresh 2)

Run from `frontend/`:

```bash
deno task dev      # Vite dev server
deno task build    # production build
deno task start    # serve built app (deno serve -A _fresh/server.js)
deno task check    # deno fmt --check && deno lint && deno check
```

### Running frontend tests

```bash
# All tests
deno test frontend/

# Single test file
deno test frontend/components/FeedItemCard_test.ts

# Filter by test name
deno test frontend/ --filter "timeAgo"
```

The frontend proxies `/api/*` requests to `http://localhost:8080` (the backend). `BACKEND_URL` is defined in `frontend/utils.ts`.

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

- `i64` for SQLite integer PKs (matches existing schema).
- DB row models derive `sqlx::FromRow`; add `serde::{Serialize, Deserialize}` and `utoipa::ToSchema` for API payloads.
- `chrono::DateTime<Utc>` for timestamps.

### Naming

- Modules and functions: `snake_case` (`fetch_feed`, `insert_feed_item_detail`).
- Structs and enums: `UpperCamelCase` (`FeedItem`, `FeedItemDetail`).
- Constants: `SCREAMING_SNAKE_CASE` (`FEED_URLS`).

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
- Use column type annotations (`"id!: i64"`) for SQLite type coercion when needed.

### Handlers

- Handlers return `Result<Json<T>, HandlerError>`.
- Annotate with `#[instrument]` for tracing and `#[utoipa::path(...)]` for OpenAPI docs.
- `HandlerError` wraps HTTP status + message; see `handlers/error.rs`.

### Testing

- Unit tests go in `#[cfg(test)] mod tests { ... }` alongside the code.
- Use `#[sqlx::test]` for DB tests (isolated migrated DB per test).
- Prefer deterministic tests. The test in `feed.rs` hits real RSS feeds and may be flaky.
- `unwrap()` is acceptable inside tests.

## Code Style -- Frontend (TypeScript / Deno Fresh)

### Formatting & Linting

- `deno fmt` for formatting (built-in, no config needed).
- `deno lint` with `"fresh"` and `"recommended"` rule tags (configured in `deno.json`).
- Run `deno task check` to verify both formatting and linting.

### Conventions

- Use TypeScript `interface` for shared types (see `types.ts`).
- Use explicit `type` imports: `import type { FeedItem } from "../types.ts"`.
- Import paths use the `@/` alias for project-root-relative imports (configured in `deno.json`).
- Preact components in `components/` (server-rendered), interactive islands in `islands/`.
- Islands use `useSignal` from `@preact/signals` for state management.
- JSX uses precompiled mode with `preact` as the import source.
- Tailwind CSS 4 for styling (via Vite plugin).

### Testing

- Test files are co-located with source: `FeedItemCard.tsx` / `FeedItemCard_test.ts`.
- Use `Deno.test()` with `@std/assert` (`assertEquals`) and `@std/testing/mock`.
- Export utility functions from components and test them separately.
- Tests are pure logic tests -- no DOM rendering tests.

## Repo Hygiene

- Do not commit secrets or `backend/.env` changes.
- Do not commit databases (`dev.db`) or build artifacts (`*/target/`, `_fresh/`).
- If you add Cursor/Copilot rules, update this file to mirror them.
