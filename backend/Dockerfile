FROM rust:latest AS builder

WORKDIR /app

COPY . .

RUN cargo build --release

# --- Runtime stage ---
FROM debian:bookworm-slim

RUN apt-get update && apt-get install -y ca-certificates libsqlite3-0 && rm -rf /var/lib/apt/lists/*

WORKDIR /app

COPY --from=builder /app/target/release/backend ./backend
COPY --from=builder /app/migrations/ ./migrations/

EXPOSE 8080

CMD ["./backend"]
