# 📰 Rust RSS Aggregator

A self-hosted RSS/Atom web aggregator with **live updates**.

Built with:

- 🦀 Rust
- ⚡ Tokio
- 🧭 Axum
- 🗄️ sqlx + SQLite
- 🌿 Deno + Fresh (frontend)
- 📡 Server-Sent Events (SSE)

---

## ✨ Overview

This project is a lightweight, self-hostable RSS/Atom aggregator that:

- Polls RSS and Atom feeds
- Stores items in SQLite
- Deduplicates at the database level
- Serves an aggregated timeline via REST
- Pushes live updates to the browser using SSE
- Uses Fresh for SSR + interactive islands

### Architecture Philosophy

> SQLite is the source of truth.  
> SSE is the live tail.

---

## 🏗 Architecture

```
Feeds (RSS / Atom)
        ↓
Poller (Tokio tasks)
        ↓
Parser (feed-rs)
        ↓
SQLite (sqlx)
        ↓
Broadcast (tokio::broadcast)
        ↓
Axum API + SSE
        ↓
Fresh Frontend
```

### Core Components

**Backend**
- Axum HTTP server
- Tokio background poll scheduler
- sqlx + SQLite (WAL mode)
- SSE endpoint with reconnect support (`Last-Event-ID`)

**Frontend**
- Fresh SSR for initial timeline
- Islands for live updates
- Proxy layer for `/api/*` including SSE streaming

---

## 🚀 Features

### Core
- Add/remove RSS or Atom feeds
- Conditional polling (ETag / Last-Modified)
- DB-level deduplication
- Cursor-based pagination
- Live updates via SSE
- Reconnect + backfill support

### Technical Highlights
- Monotonic `feed_items.id` for:
  - Cursor pagination
  - SSE replay
- `INSERT OR IGNORE` dedupe
- Broadcast channel fanout
- DB-backed replay on reconnect

Note: the implementation stores item metadata in `feed_items` and heavier fields in
`feed_item_details` (1:1), which keeps timeline reads fast while preserving full
content for detail views.

---

## 🧱 MVP Feature List

### Backend

#### Feed Management
- [x] Add feed (DB: upsert by URL)
- [x] List feeds (DB)
- [x] Enable/disable feed (DB)
- [x] Delete feed (DB)

#### Polling
- [x] Scheduled polling per feed
- [x] Conditional GET (ETag / 304 support)
- [ ] Backoff on failure
- [ ] Jittered intervals

#### Items
- [x] Store normalized items (`feed_items` + `feed_item_details`)
- [x] Deduplicate via `UNIQUE(feed_id, external_id)`
- [ ] Cursor pagination
- [x] Filter by feed (DB)

#### Live Updates
- [ ] `GET /api/events` (SSE)
- [x] KeepAlive pings
- [ ] Reconnect via `Last-Event-ID`
- [ ] DB backfill on reconnect
- [ ] Broadcast lag recovery

### Frontend (Fresh)

- [x] SSR aggregated timeline
- [x] SSE island for live updates
- [x] Add feed form
- [x] Basic per-feed filtering

---

## 🗺 Roadmap

### Phase 1 — MVP (Current Goal)
- [x] Core ingest pipeline
- [x] SQLite schema + migrations
- [x] REST + SSE API
- [x] Fresh SSR + live island

### Phase 2 — Usability
- Mark as read/unread
- Unread counter
- Per-feed views
- Manual refresh endpoint
- Basic search (LIKE-based)

### Phase 3 — Power Features
- OPML import/export
- Feed groups/folders
- Retention policies
- Full-text search (SQLite FTS5)
- Combined RSS export

### Phase 4 — Scaling & Ops
- Postgres support
- Horizontal SSE scaling
- Metrics endpoint
- Feed health dashboard

---

## 🛠 Development Setup

### Requirements

- Rust (stable)
- SQLite
- Deno ≥ 2.x
- Fresh ≥ 2.x

---

### Backend Setup

```bash
git clone <repo>
cd backend
```

The backend uses `DATABASE_URL` (SQLite) and defaults to `sqlite:dev.db` if unset.

Example:

```bash
export DATABASE_URL='sqlite:dev.db'
```

Run:

```bash
cargo run
```

SQLite will be created automatically if it does not exist.

---

### Frontend Setup

```bash
cd frontend
deno task start
```

Frontend runs on:

```
http://localhost:8000
```

Backend runs on:

```
http://localhost:8080
```

Fresh proxies `/api/*` to backend.

---

## 📡 API Overview

### Feeds

```
GET    /api/feeds
POST   /api/feeds
PATCH  /api/feeds/:id
DELETE /api/feeds/:id
```

### Items

```
GET /api/items?limit=50&cursor=123
GET /api/items?feed_id=1
GET /api/items/:id
```

### Live Updates (SSE)

```
GET /api/events
```

Example SSE frame:

```
id: 345
event: item
data: { ...json... }
```

Reconnect automatically handled via `Last-Event-ID`.

`id` should correspond to `feed_items.id` to support monotonic cursors and replay.

---

## 🗄 Database Schema (MVP)

### feeds

- id (INTEGER PRIMARY KEY)
- url (UNIQUE)
- title
- site_url
- etag
- last_modified
- poll_interval_seconds
- is_enabled
- last_checked_at
- last_success_at
- failure_count

### feed_items

- id (INTEGER PRIMARY KEY)
- feed_id (FK -> feeds.id)
- external_id
- title
- url
- inserted_at

Constraints:

```
UNIQUE(feed_id, external_id)
```

### feed_item_details

- id (INTEGER PRIMARY KEY)
- feed_item_id (UNIQUE FK -> feed_items.id)
- summary
- content
- author
- published_at

Notes:

- One detail row per item (`UNIQUE(feed_item_id)`).
- `ON DELETE CASCADE` from `feed_items` to `feed_item_details`.

---

## 🧪 Testing Strategy

### Unit Tests
- Dedupe logic
- External ID selection
- Poll scheduling logic

### Integration Tests
- Mock feed server:
  - 200 → insert
  - 304 → skip
  - 200 with new items → insert + broadcast

### SSE Tests
- Connect
- Receive event
- Disconnect
- Reconnect with `Last-Event-ID`
- Verify backfill

---

## 📦 Deployment Model

Single binary + SQLite file.

Recommended:
- Run behind reverse proxy
- Enable SQLite WAL mode
- Regular SQLite backups

---

## 📈 Design Principles

- Keep it simple
- SQLite first
- Database is canonical history
- SSE is best-effort live view
- Recover from lag using DB replay
- No unnecessary external infrastructure

---

## 🧠 Why This Stack?

- **Axum**: ergonomic async HTTP + built-in SSE
- **Tokio**: robust async runtime + broadcast channels
- **sqlx**: compile-time query validation + migrations
- **SQLite**: perfect for single-user self-hosting
- **Fresh**: SSR + islands = ideal for timeline + live updates

---

## 📜 License

MIT
