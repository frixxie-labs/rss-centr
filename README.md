# Rust RSS Aggregator

A self-hosted RSS/Atom web aggregator with live updates.

Built with Rust (Axum + Tokio + sqlx), PostgreSQL, and Deno Fresh 2.

---

## Overview

A lightweight, self-hostable RSS/Atom aggregator that:

- Polls RSS and Atom feeds on a configurable schedule
- Stores items in PostgreSQL with DB-level deduplication
- Serves an aggregated timeline via REST API
- Pushes live updates to the browser using Server-Sent Events (SSE)
- Supports full-text search across items and feed metadata
- Exposes Prometheus metrics and an OpenAPI spec
- Uses Fresh 2 for SSR + interactive islands

### Architecture

```
Feeds (RSS / Atom)
        |
Poller (Tokio tasks)
        |
Parser (feed-rs)
        |
PostgreSQL (sqlx)
        |
Broadcast (tokio::broadcast)
        |
Axum API + SSE
        |
Fresh 2 Frontend
```

**Backend**: Axum HTTP server, Tokio background poll scheduler, sqlx + PostgreSQL, SSE endpoint with reconnect support (`Last-Event-ID`).

**Frontend**: Fresh 2 SSR for initial page loads, Preact islands for interactivity, Tailwind CSS 4 for styling, proxy layer for `/api/*` including SSE streaming.

---

## Features

### Core
- Add/remove/enable/disable RSS or Atom feeds
- Conditional polling (ETag / Last-Modified / 304 support)
- Backoff on failure with jittered intervals
- DB-level deduplication via `UNIQUE(feed_id, external_id)`
- Live updates via SSE with reconnect + DB backfill
- Full-text search (PostgreSQL GIN indexes)
- Manual feed ingest trigger
- Word frequency index from item titles
- Prometheus metrics at `/metrics`
- OpenAPI spec at `/openapi`
- Health and liveness endpoints

### Technical Highlights
- Monotonic `feed_items.id` (BIGSERIAL) for SSE replay
- `ON CONFLICT DO NOTHING` deduplication
- Broadcast channel fanout for SSE
- DB-backed replay on reconnect via `Last-Event-ID`
- Split storage: `feed_items` (metadata) + `feed_item_details` (content) keeps timeline reads fast
- GIN indexes for full-text search on items, details, and feeds

---

## Development Setup

### Requirements

- Rust (stable, edition 2024)
- PostgreSQL 17+
- Deno 2.x+
- Docker Compose

### Quick Start

```bash
# Start the database
docker compose up -d db

# Backend (listens on http://localhost:8080)
export DATABASE_URL='postgres://postgres:postgres@localhost:5432/rss_centr'
cargo run --manifest-path backend/Cargo.toml

# Frontend (listens on http://localhost:8000, proxies /api/* to backend)
cd frontend
deno task dev
```

Migrations are applied automatically on backend startup.

### Backend Commands

From the repo root:

```bash
cargo build   --manifest-path backend/Cargo.toml
cargo run     --manifest-path backend/Cargo.toml
cargo test    --manifest-path backend/Cargo.toml
cargo fmt     --manifest-path backend/Cargo.toml -- --check
cargo clippy  --manifest-path backend/Cargo.toml --all-targets --all-features -- -D warnings
```

Or use the justfile in `backend/`:

```bash
just check    # cargo check
just lint     # cargo clippy
just test     # cargo test
just build    # prepare + check + build
just prepare  # sqlx database create + migrate + prepare
```

### Frontend Commands

From `frontend/`:

```bash
deno task dev      # Vite dev server
deno task build    # production build
deno task start    # serve built app
deno task check    # fmt --check + lint + type check
```

### Backend CLI Options

```
--host <HOST>                        Listen address [default: 0.0.0.0:8080]
--db-url <URL>                       Database URL [env: DATABASE_URL]
--log-level <LEVEL>                  Log level [default: info]
--scheduler-interval-seconds <SECS>  Poll interval [default: 30]
```

---

## API

### Feeds

```
GET    /api/feeds                    List all feeds
POST   /api/feeds                    Add feed (upsert by URL)
GET    /api/feeds/{feed_id}          Get single feed
PUT    /api/feeds/{feed_id}          Enable/disable feed
DELETE /api/feeds/{feed_id}          Delete feed
POST   /api/feeds/{feed_id}/ingest   Manually trigger feed ingest (202)
```

### Items

```
GET    /api/items/latest             Latest items with details
                                     Query: limit, feed_id, q (search)
GET    /api/items/{item_id}          Single item
GET    /api/items/{item_id}/detail   Item detail (summary, content, author)
GET    /api/feeds/{feed_id}/items    Items for a specific feed
```

### Title Index

```
GET    /api/feeds/index              Full title word frequency index
GET    /api/feeds/index/today        Recent (24h) title word frequency index
```

### Live Updates (SSE)

```
GET    /api/items/stream             SSE stream (query: last_event_id)
```

SSE frame format:

```
id: 345
event: feed_item
data: { ...json... }
```

Reconnect is handled via `Last-Event-ID` header with bounded DB backfill. The
server emits `event: replay_done` after catch-up; its payload includes
`replayed`, `limited`, and `last_event_id`.

### Operational

```
GET    /status/ping                  Liveness check
GET    /status/health                Health check (DB connectivity, 200/503)
GET    /metrics                      Prometheus metrics
GET    /openapi                      OpenAPI JSON spec
```

---

## Frontend Pages

| Path | Description |
|------|-------------|
| `/` | Timeline with live SSE updates |
| `/items` | All items view (limit 500) |
| `/feeds` | Feed management (add/enable/disable/delete) |
| `/index-words` | Word cloud from title index |

---

## Database Schema

All primary keys are `BIGSERIAL` (`BIGINT` auto-increment).

### feeds

| Column | Type | Notes |
|--------|------|-------|
| id | BIGSERIAL | PK |
| url | TEXT | NOT NULL, UNIQUE |
| title | TEXT | nullable |
| site_url | TEXT | nullable |
| etag | TEXT | nullable |
| last_modified | TEXT | nullable |
| poll_interval_seconds | BIGINT | NOT NULL, default 300 |
| is_enabled | BOOLEAN | NOT NULL, default TRUE |
| last_checked_at | TIMESTAMPTZ | nullable |
| last_success_at | TIMESTAMPTZ | nullable |
| failure_count | BIGINT | NOT NULL, default 0 |

### feed_items

| Column | Type | Notes |
|--------|------|-------|
| id | BIGSERIAL | PK |
| feed_id | BIGINT | NOT NULL, FK -> feeds(id) ON DELETE CASCADE |
| external_id | TEXT | NOT NULL |
| title | TEXT | NOT NULL |
| url | TEXT | NOT NULL |
| inserted_at | TIMESTAMPTZ | NOT NULL, default CURRENT_TIMESTAMP |

Constraint: `UNIQUE(feed_id, external_id)`

### feed_item_details

| Column | Type | Notes |
|--------|------|-------|
| id | BIGSERIAL | PK |
| feed_item_id | BIGINT | NOT NULL, UNIQUE, FK -> feed_items(id) ON DELETE CASCADE |
| summary | TEXT | NOT NULL |
| content | TEXT | NOT NULL |
| author | TEXT | NOT NULL |
| published_at | TIMESTAMPTZ | NOT NULL |

### Indexes

- `idx_feed_items_feed_id_inserted_at_id` on `(feed_id, inserted_at DESC, id DESC)`
- `idx_feed_items_inserted_at_id` on `(inserted_at DESC, id DESC)`
- `idx_feeds_is_enabled_id` on `(is_enabled, id)`
- GIN full-text search indexes on `feed_items`, `feed_item_details`, and `feeds`

---

## Deployment

### Docker Compose

```bash
docker compose up -d
```

Runs three services: `db` (Postgres 17), `backend` (port 8080), `frontend` (port 8000).

Kubernetes manifests are available in `release/` (Kustomize-based).

### CI/CD

GitHub Actions and GitLab CI pipelines are configured for build, test, and container image deployment (on tags).

---

## Design Principles

- PostgreSQL is the source of truth; SSE is the live tail
- Recover from lag using DB replay
- Keep it simple -- no unnecessary external infrastructure
- Database is canonical history

---

## License

MIT
