# AGENTS.md

This repository currently contains a Rust backend in `backend/`.
There are no Cursor rules (`.cursor/rules/` or `.cursorrules`) and no Copilot instructions (`.github/copilot-instructions.md`) in this repo as of today.

## Version Control (jj)

This repo uses Jujutsu (`jj`) for day-to-day version control. The repository is also a Git repo on disk for compatibility, but prefer `jj` commands and workflows.

Common commands:

```bash
jj status
jj diff
jj log

# Add/adjust change description (commit message)
jj describe -m "<message>"
```

## Commands (Build / Lint / Test)

Run from repo root (works regardless of your current directory):

```bash
# Build
cargo build --manifest-path backend/Cargo.toml

# Run
cargo run --manifest-path backend/Cargo.toml

# Format
cargo fmt --manifest-path backend/Cargo.toml
cargo fmt --manifest-path backend/Cargo.toml -- --check

# Lint
cargo clippy --manifest-path backend/Cargo.toml --all-targets --all-features
cargo clippy --manifest-path backend/Cargo.toml --all-targets --all-features -- -D warnings

# Test (all)
cargo test --manifest-path backend/Cargo.toml
```

Run from `backend/` (shorter):

```bash
cargo build
cargo run
cargo fmt
cargo clippy --all-targets --all-features
cargo test
```

### Running a single test

Preferred patterns:

```bash
# Exact test name (substring match)
cargo test --manifest-path backend/Cargo.toml test_insert_feed_item

# Fully qualified path (best when names collide)
cargo test --manifest-path backend/Cargo.toml feed::feed_item::tests::test_insert_feed_item

# Run a single test and show stdout
cargo test --manifest-path backend/Cargo.toml test_insert_feed_item -- --nocapture
```

Helpful test tooling:

```bash
# List tests
cargo test --manifest-path backend/Cargo.toml -- --list

# Run only lib/unit tests (if a lib crate is added later)
cargo test --manifest-path backend/Cargo.toml --lib
```

## Environment & Database (sqlx + SQLite)

- `backend/` uses `sqlx` macros (`query!`, `query_as!`). These perform compile-time SQL checking and typically require `DATABASE_URL` to be set in the environment during `cargo build` / `cargo test`.
- A local dev value exists in `backend/.env`:
  - `DATABASE_URL=sqlite:dev.db`
- Rust/sqlx does not automatically load `.env` at compile time. If builds/tests fail with missing `DATABASE_URL`, export it in your shell:

```bash
export DATABASE_URL='sqlite:dev.db'
cargo test --manifest-path backend/Cargo.toml
```

Migrations live in `backend/migrations/`. Tests using `#[sqlx::test]` are expected to run with migrations applied (sqlx test harness manages this).

If you use `sqlx-cli` locally, typical commands look like:

```bash
# Install once (optional)
cargo install sqlx-cli

# Apply migrations to your dev DB
sqlx migrate run --source backend/migrations
```

## Code Style (Repository Conventions)

This section captures conventions already present in the codebase. Prefer following existing patterns over introducing new ones.

### Formatting

- Use `rustfmt` (default settings). Do not hand-format.
- Keep lines reasonably short; wrap fluent chains as rustfmt does.
- Use raw strings for SQL (`r#"..."#`) with consistent indentation.

### Imports

- Group imports roughly as:
  - std
  - external crates
  - crate modules
- Prefer explicit imports (e.g. `use anyhow::{Context, Result};`) over glob imports.
- Keep `use` blocks small and local to the module.

### Types and Data Modeling

- Use strong types for IDs when possible, but keep consistency with existing schema (`i64` for SQLite integer PKs).
- For DB row models, derive:
  - `sqlx::FromRow` (already used)
  - `serde::{Serialize, Deserialize}` when used in API payloads
- Use `chrono::DateTime<Utc>` for timestamps (already used).

### Naming

- Modules and functions: `snake_case` (`fetch_feed`, `insert_feed_item_detail`).
- Structs and enums: `UpperCamelCase` (`FeedItem`, `FeedItemDetail`).
- Constants: `SCREAMING_SNAKE_CASE` (`FEED_URLS`).

### Error handling

- Use `anyhow::Result<T>` for fallible operations.
- Add context at boundaries:
  - Network/IO/DB calls should use `.with_context(|| ...)` with actionable messages.
- Use `anyhow::bail!` for domain errors like “not found” (see `delete_*` functions).
- Avoid `unwrap()` / `expect()` outside tests.

### Async / Tokio

- Prefer passing a `reqwest::Client` around (already done in `fetch_feed`).
- Keep async functions cancellation-safe when possible (avoid holding locks across `.await`).

### SQLx usage

- Prefer compile-time checked macros (`query!`, `query_as!`) as the code already does.
- Keep SQL close to the function that uses it; avoid building SQL with string concatenation.
- Always bind parameters (no string interpolation into SQL).
- For “no rows affected” cases, convert to a domain error (see `rows_affected()` checks).

### Testing

- Keep unit tests near the code (`#[cfg(test)] mod tests { ... }`).
- Use `#[sqlx::test]` for DB tests; it provides an isolated database per test.
- Prefer deterministic tests. Note: `backend/src/feed/feed.rs` contains a networked test that fetches real feeds; it may be flaky/offline-sensitive.

## Repo Hygiene for Agents

- Do not commit secrets. Avoid committing `backend/.env` changes.
- Avoid committing databases and build artifacts (`backend/dev.db`, `*/target/`).
- If you add new tooling rules (Cursor/Copilot), update this file to mirror them.
