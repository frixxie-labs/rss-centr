FROM rust:trixie AS builder
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates curl clang pkg-config libssl-dev \
    && rm -rf /var/lib/apt/lists/*
RUN curl -L --proto '=https' --tlsv1.2 -sSf https://raw.githubusercontent.com/cargo-bins/cargo-binstall/main/install-from-binstall-release.sh | bash \
    && cargo binstall -y --locked cargo-leptos@^0.3
RUN rustup target add wasm32-unknown-unknown
WORKDIR /app
COPY . .
WORKDIR /app/frontend
RUN cargo leptos build --release --cargo-locked

FROM debian:trixie-slim AS runner
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/*
WORKDIR /app
COPY --from=builder /app/target/release/rss-centr-frontend ./server
COPY --from=builder /app/target/site ./site
ENV IP=0.0.0.0 PORT=8000 LEPTOS_SITE_ROOT=site LEPTOS_ENV=PROD LEPTOS_OUTPUT_NAME=rss-centr-frontend
USER 65532:65532
EXPOSE 8000
CMD ["./server"]
