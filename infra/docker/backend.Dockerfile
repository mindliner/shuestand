# syntax=docker/dockerfile:1
FROM rust:1.75 AS builder
WORKDIR /app

# Cache deps first
COPY backend/Cargo.toml backend/Cargo.lock ./backend/
COPY backend/src ./backend/src
COPY vendor ./vendor
WORKDIR /app/backend
RUN cargo build --release

FROM debian:bookworm-slim
RUN useradd --create-home shuestand
WORKDIR /app
COPY --from=builder /app/backend/target/release/backend /usr/local/bin/shuestand-backend
COPY backend/migrations ./migrations
ENV RUST_LOG=info
USER shuestand
EXPOSE 8080
CMD ["shuestand-backend"]
