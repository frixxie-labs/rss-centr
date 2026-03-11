FROM docker.io/library/rust:latest

WORKDIR /app

ENV SQLX_OFFLINE=true

COPY . .

RUN cargo build --release

EXPOSE 8080

CMD ["./target/release/backend"]
