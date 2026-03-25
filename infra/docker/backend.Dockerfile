# syntax=docker/dockerfile:1
FROM rust:1.94 AS builder
WORKDIR /app

RUN apt-get update \
    && apt-get install -y --no-install-recommends protobuf-compiler \
    && rm -rf /var/lib/apt/lists/*

# Cache deps first
COPY backend/Cargo.toml backend/Cargo.lock ./backend/
COPY backend/src ./backend/src
COPY backend/migrations ./backend/migrations
COPY vendor ./vendor
WORKDIR /app/backend
RUN cargo build --release

FROM debian:bookworm-slim
RUN useradd --create-home shuestand
WORKDIR /app
COPY --from=builder /app/backend/target/release/backend /usr/local/bin/shuestand-backend
COPY backend/migrations ./migrations
RUN mkdir -p /data/cashu \
    && chown -R shuestand:shuestand /data
ENV RUST_LOG=info
USER shuestand
EXPOSE 8872
CMD ["shuestand-backend"]
