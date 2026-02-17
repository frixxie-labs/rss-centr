FROM rust:latest AS builder

WORKDIR /app

# Install SQLite dev libraries for sqlx compile-time checks
RUN apt-get update && apt-get install -y libsqlite3-dev && rm -rf /var/lib/apt/lists/*

# Copy manifests first to cache dependency builds
COPY Cargo.toml Cargo.lock ./

# Create a dummy main.rs to build dependencies
RUN mkdir src && echo "fn main() {}" > src/main.rs

# We need the migrations and a database for sqlx compile-time checks
COPY migrations/ migrations/
RUN DATABASE_URL="sqlite:build.db" cargo install sqlx-cli --no-default-features --features sqlite \
    && DATABASE_URL="sqlite:build.db" sqlx database create \
    && DATABASE_URL="sqlite:build.db" sqlx migrate run

# Build dependencies only (cached unless Cargo.toml/Cargo.lock change)
RUN DATABASE_URL="sqlite:build.db" cargo build --release && rm -rf src

# Copy actual source code
COPY src/ src/

# Touch main.rs so cargo rebuilds the real binary
RUN touch src/main.rs

# Build the real binary
RUN DATABASE_URL="sqlite:build.db" cargo build --release

# --- Runtime stage ---
FROM debian:bookworm-slim

RUN apt-get update && apt-get install -y ca-certificates libsqlite3-0 && rm -rf /var/lib/apt/lists/*

WORKDIR /app

COPY --from=builder /app/target/release/backend ./backend
COPY --from=builder /app/migrations/ ./migrations/

EXPOSE 8080

ENTRYPOINT ["./backend"]
