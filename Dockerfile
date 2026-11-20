# ============================================================
#  ТАНЧИКИ — сборка и запуск в Docker
#
#  Статика вшивается в бинарник на этапе сборки (rust-embed),
#  поэтому в финальный образ попадает только один файл.
# ============================================================

# ---------- Этап 1: сборка ----------
FROM rust:1-slim-bookworm AS builder

WORKDIR /app

# Сначала копируем манифесты — слой с зависимостями кешируется отдельно.
COPY Cargo.toml Cargo.lock ./
RUN mkdir -p src \
    && echo 'fn main() {}' > src/main.rs \
    && cargo build --release \
    && rm -rf src

# Теперь исходники целиком и пересборка уже с зависимостями.
COPY src ./src
COPY static ./static
# touch меняет время файла и заставляет cargo пересобрать крейт.
RUN touch src/main.rs && cargo build --release

# ---------- Этап 2: рантайм ----------
FROM debian:bookworm-slim

RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates curl \
    && rm -rf /var/lib/apt/lists/*

COPY --from=builder /app/target/release/tanchiki /usr/local/bin/tanchiki

ENV TANCHIKI_BIND=0.0.0.0:3000 \
    RUST_LOG=info

EXPOSE 3000

# Проверка живости: /healthz отдаёт «ok».
HEALTHCHECK --interval=30s --timeout=3s --start-period=5s --retries=3 \
    CMD curl -fsS http://127.0.0.1:3000/healthz || exit 1

CMD ["tanchiki"]
