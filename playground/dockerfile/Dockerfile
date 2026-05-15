# Multi-stage build for a Rust app.
FROM rust:1.78 AS builder

WORKDIR /app

COPY Cargo.toml Cargo.lock ./
COPY src ./src

RUN cargo build --release --locked

# ---

FROM debian:bookworm-slim AS runtime

RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/*

ARG APP_USER=app
RUN useradd --create-home --shell /bin/bash "${APP_USER}"

WORKDIR /home/${APP_USER}

COPY --from=builder /app/target/release/app /usr/local/bin/app

USER ${APP_USER}

ENV RUST_LOG=info \
    APP_PORT=8080

EXPOSE 8080

HEALTHCHECK --interval=30s --timeout=3s --retries=3 \
    CMD curl -fsS http://localhost:${APP_PORT}/health || exit 1

ENTRYPOINT ["/usr/local/bin/app"]
CMD ["--config", "/etc/app/config.toml"]
