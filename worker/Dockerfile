FROM docker.io/library/rust:bookworm AS builder

WORKDIR /app

COPY . .

RUN cargo build --release --package worker

FROM docker.io/library/debian:bookworm-slim AS runner

RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app

COPY --from=builder /app/target/release/worker /app/worker

CMD ["./worker"]
