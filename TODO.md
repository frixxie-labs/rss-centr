# TODO

- [x] Add worker image publishing and worker deployment to release pipelines/manifests so tagged/Kubernetes deployments continue polling feeds.
- [x] Restrict worker feed-update queue endpoints from public `/api` access
- [x] Include feed metadata and cadence updates in the worker completion flow, preserving `title`, `site_url`, and adaptive poll interval behavior.
- [x] Prevent or clearly reject manual `Fetch now` for paused feeds so queued work cannot silently no-op.
- [x] Only update `last_inserted_at` when new feed items are actually inserted, not on not-modified or all-dedup completions.
- [x] Update Docker ignore rules for root build context to exclude local env/secrets such as `backend/.env` and `.env`.
