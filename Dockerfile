FROM rust:1.95-bookworm AS builder

RUN apt-get update \
    && apt-get install -y --no-install-recommends clang cmake pkg-config libsqlite3-dev \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /build
COPY Cargo.toml Cargo.lock ./
COPY src ./src
RUN cargo build --release --locked

FROM debian:bookworm-slim

RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates curl sqlite3 libsqlite3-0 \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --system --uid 10001 --create-home --home-dir /app vpush

WORKDIR /app
COPY --from=builder /build/target/release/vpush /app/vpush
COPY static /app/static
RUN mkdir -p /app/data && chown -R vpush:vpush /app

USER vpush
ENV HOST=0.0.0.0 \
    PORT=8000 \
    VPUSH_DB=/app/data/vpush.db \
    VPUSH_STATIC=/app/static
EXPOSE 8000
HEALTHCHECK --interval=30s --timeout=5s --start-period=10s --retries=3 \
    CMD ["/usr/bin/curl", "-fsS", "http://127.0.0.1:8000/"]

ENTRYPOINT ["/app/vpush"]
