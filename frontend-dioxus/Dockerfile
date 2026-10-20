FROM rust:trixie AS builder
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates curl clang pkg-config libssl-dev \
    && rm -rf /var/lib/apt/lists/*
RUN curl -L --proto '=https' --tlsv1.2 -sSf https://raw.githubusercontent.com/cargo-bins/cargo-binstall/main/install-from-binstall-release.sh | bash \
    && cargo binstall -y --locked dioxus-cli@^0.7
RUN rustup target add wasm32-unknown-unknown
WORKDIR /app
COPY . .
WORKDIR /app/frontend-dioxus
# Use the committed assets/styles.css; regenerating it here changes its hash after the server binary embeds the old one.
RUN sed -i '/^tailwind_\(input\|output\)/d' Dioxus.toml
RUN dx build --web --release --profile wasm-release --package rss-centr-frontend --no-default-features --locked

FROM debian:trixie-slim AS runner
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/*
WORKDIR /app
COPY --from=builder /app/target/dx/rss-centr-frontend/release/web/ ./
ENV IP=0.0.0.0 PORT=8000
USER 65532:65532
EXPOSE 8000
CMD ["./server"]
