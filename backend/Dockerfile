FROM docker.io/library/rust:bookworm AS builder

WORKDIR /app

ENV SQLX_OFFLINE=true

COPY . .

RUN cargo build --release

FROM docker.io/library/debian:bookworm-slim AS runner


RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app

COPY --from=builder /app/target/release/backend /app/backend

EXPOSE 8080

CMD ["./backend"]
