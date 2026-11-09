# MiSFY DropCard — one image: the Rust server plus the built frontend.

# 1) Frontend (React + Bryntum trial + AG Grid, from public npm)
FROM node:22-bookworm-slim AS web
WORKDIR /app/web
COPY web/package.json web/package-lock.json ./
RUN npm ci --no-audit --no-fund
COPY web/ ./
RUN npm run build

# 2) Server (dependencies are built in their own layer so code changes rebuild fast)
FROM rust:1.95-slim-bookworm AS server
WORKDIR /app
COPY Cargo.toml Cargo.lock ./
RUN mkdir src && echo 'fn main() {}' > src/main.rs && cargo build --release --locked && rm -rf src
COPY src ./src
COPY migrations ./migrations
RUN touch src/main.rs && cargo build --release --locked

# 3) Runtime
FROM debian:bookworm-slim
RUN apt-get update \
 && apt-get install -y --no-install-recommends ca-certificates \
 && rm -rf /var/lib/apt/lists/* \
 && useradd --system --uid 10001 app
WORKDIR /app
COPY --from=server /app/target/release/misfy-dropcard /usr/local/bin/misfy-dropcard
COPY --from=web /app/web/dist ./web/dist
ENV WEB_DIR=/app/web/dist \
    PORT=8080 \
    COOKIE_SECURE=1 \
    RUST_LOG=info,sqlx=warn,tower_http=info
USER app
EXPOSE 8080
CMD ["misfy-dropcard", "serve"]
