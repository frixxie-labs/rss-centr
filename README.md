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
- [x] Backoff on failure
- [x] Jittered intervals

#### Items
- [x] Store normalized items (`feed_items` + `feed_item_details`)
- [x] Deduplicate via `UNIQUE(feed_id, external_id)`
- [ ] Cursor pagination (not yet implemented)
- [x] Filter by feed (DB)

#### Live Updates
- [x] `GET /api/items/stream` (SSE)
- [x] KeepAlive pings
- [x] Reconnect via `Last-Event-ID`
- [x] DB backfill on reconnect
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
GET /api/items/stream
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

## 📋 Known Issues & TODOs

Issues identified via code review, grouped by priority.

### High Priority

- [ ] **Fragile 404 detection** — `backend/src/handlers/error.rs:57` uses `starts_with("no ")` string matching to decide 404 vs 500. Replace with a typed error enum or dedicated `NotFound` variant.
- [ ] **Scheduler dies on transient DB errors** — `backend/src/background_tasks.rs:85` propagates errors with `?`, permanently killing the scheduler on any transient SQLite failure. Wrap in a retry loop with logging.
- [ ] **Unbounded metric cardinality** — `backend/src/handlers/mod.rs:37` uses raw URI path as a Prometheus label for unmatched routes. Bot traffic creates infinite label combinations. Use a fixed label like `"unmatched"`.

### Medium Priority

- [ ] **No default limit on `fetchLatestItems`** — `backend/src/handlers/items.rs:62` returns ALL items with details when no `limit` is provided. Add a default cap.
- [ ] **Title index rebuilds from scratch per request** — `backend/src/handlers/feed_title_index.rs:15-27` loads all items into memory on each call. Add caching or incremental build.
- [ ] **SSE replay has no cap** — `backend/src/handlers/sse.rs:42` replays the entire item history for a very old `Last-Event-ID`. Add a maximum replay window.
- [ ] **`COALESCE` prevents clearing stale cache headers** — `backend/src/feed/feed_subscription.rs:225-228` means old ETag/Last-Modified values persist even when a feed stops sending them.
- [ ] **No URL format validation on feed creation** — `backend/src/handlers/feeds.rs:69-71` only checks for empty string. Use `Url::parse()`.
- [ ] **`NotModified` triggers exponential backoff** — `backend/src/feed/ingest.rs:71-72` doubles poll interval on 304. Infrequently-updated feeds quickly hit MAX_POLL_INTERVAL. Consider using cadence-based intervals instead.
- [ ] **Navigation duplicated across all routes** — `frontend/routes/index.tsx`, `items.tsx`, `feeds.tsx`, `index-words.tsx` each have ~24 lines of identical nav markup. Extract to a shared component.
- [ ] **`localStorage` access without try/catch** — `frontend/islands/Timeline.tsx:79,112-113` can throw in restricted contexts or when quota is exceeded.
- [ ] **`updateFeedEnabled` retry hack** — `frontend/api.ts:85-118` retries with a different field name on 400 status. Resolve at the API contract level.
- [ ] **No pagination on items page** — `frontend/routes/items.tsx:20` calls `fetchLatestItems()` with no limit.

### Low Priority

- [ ] **`nom` used only for whitespace splitting** — `backend/src/feed/feed_title_index.rs:297-303` could use `split_whitespace()` and eliminate the `nom` dependency.
- [ ] **Typo: `occurences`** — `backend/src/feed/feed_title_index.rs:334` should be `occurrences`. Appears in JSON API output — fixing is a breaking change (consider serde rename).
- [ ] **Custom `LogLevel` duplicates `tracing::Level`** — `backend/src/main.rs:24-58`.
- [ ] **Metric name not namespaced** — `backend/src/handlers/mod.rs:46` uses `"handler"` instead of `rss_centr_handler_*`.
- [ ] **Error responses are plain text** — `backend/src/handlers/error.rs:47-49` is inconsistent with the JSON API convention.
- [ ] **Dead code: `list_enabled_feeds`** — `backend/src/feed/feed_subscription.rs:23` is never called.
- [ ] **Missing `#[utoipa::path]` annotations** — `backend/src/handlers/feed_title_index.rs` and `sse.rs` have undocumented endpoints.
- [ ] **Duplicated sort utilities** — `frontend/islands/Timeline.tsx:18-27` and `FeedItemsView.tsx:12-21` share identical `effectiveDate`/`sortByNewest` functions. Extract to shared module.
- [ ] **SSR/client locale mismatch** — `frontend/components/FeedItemCard.tsx:14,52` uses `toLocaleString()` which may differ between server and client, risking hydration mismatches.
- [ ] **No SSRF protection** — `create_feed` accepts any URL including internal network addresses. Consider URL filtering.
- [ ] **`total_items: i32` inconsistency** — `backend/src/feed/feed_title_index.rs:355` uses `i32` while the rest of the codebase uses `i64`.
- [ ] **Unused re-export** — `backend/src/feed/mod.rs:7` re-exports `ingest_feed_url` but it's never used externally.

---

## 📜 License

MIT
