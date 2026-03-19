FROM rust:1.90-alpine AS builder
RUN apk add --no-cache musl-dev
WORKDIR /app
COPY Cargo.toml Cargo.lock ./
RUN mkdir src && echo "fn main() {}" > src/main.rs && \
    cargo build --release && rm -rf src
COPY src ./src
RUN touch src/main.rs && cargo build --release

FROM alpine:3.21
RUN apk add --no-cache ca-certificates && adduser -D appuser
COPY --from=builder /app/target/release/signal-bot-crawly /usr/local/bin/signal-bot
USER appuser
CMD ["signal-bot"]
