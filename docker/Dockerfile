# ---- 前端构建 ----
FROM node:20-slim AS web
WORKDIR /src
RUN corepack enable
COPY web/package.json web/pnpm-lock.yaml ./
RUN pnpm install --frozen-lockfile
COPY web/ ./
RUN pnpm build

# ---- 后端构建 ----
FROM rust:1-slim AS build
WORKDIR /src
RUN apt-get update && apt-get install -y --no-install-recommends pkg-config && rm -rf /var/lib/apt/lists/*
COPY Cargo.toml Cargo.lock ./
COPY crates/ crates/
COPY migrations/ migrations/
# docs_corpus.rs 用 include_str! 把产品内文档编进二进制，所以它是后端的源码依赖，
# 不只是前端资源——漏掉这行，cargo build 直接报 couldn't read。
COPY web/src/docs/ web/src/docs/
RUN cargo build --release -p utopia-server

# ---- 运行 ----
FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates && rm -rf /var/lib/apt/lists/*
WORKDIR /app
COPY --from=build /src/target/release/utopia-server /usr/local/bin/utopia-server
COPY --from=web /src/dist /app/web-dist
ENV UTOPIA_WEB_DIST=/app/web-dist
EXPOSE 8080
CMD ["utopia-server"]
