# syntax=docker/dockerfile:1

# Keep this version aligned with rust-toolchain.toml.
ARG RUST_VERSION=1.99.1

FROM node:22-bookworm-slim AS web-builder
WORKDIR /web
COPY apps/web/package.json apps/web/package-lock.json ./
RUN npm ci
COPY apps/web/ ./
RUN npm run build

FROM rust:${RUST_VERSION}-slim-bookworm AS devtool
WORKDIR /workspace
ENV SQLX_OFFLINE=true
ENV CARGO_BUILD_JOBS=4
RUN rustup component add rustfmt clippy
COPY . .

FROM devtool AS builder
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/usr/local/cargo/git \
    --mount=type=cache,target=/workspace/target \
    cargo build --locked --release --package turnesta-api --bin turnesta-api \
    && mkdir -p /out \
    && cp target/release/turnesta-api /out/turnesta-api

FROM debian:bookworm-slim AS runtime
RUN apt-get update \
    && apt-get install --yes --no-install-recommends ca-certificates curl \
    && rm -rf /var/lib/apt/lists/* \
    && groupadd --gid 10001 turnesta \
    && useradd --uid 10001 --gid turnesta --no-create-home --shell /usr/sbin/nologin turnesta
WORKDIR /app
COPY --from=builder --chown=10001:10001 /out/turnesta-api /usr/local/bin/turnesta-api
COPY --from=web-builder --chown=10001:10001 /web/dist /app/web
USER 10001:10001
EXPOSE 8080
STOPSIGNAL SIGTERM
# Exec form delivers SIGTERM directly to the Rust process. Migrations are embedded.
ENTRYPOINT ["/usr/local/bin/turnesta-api"]
CMD ["serve"]
