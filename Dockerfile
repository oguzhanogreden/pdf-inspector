# ---- Build stage ----
FROM rust:1.90-bookworm AS builder

WORKDIR /app

COPY Cargo.toml ./
COPY src/ src/
COPY external/ external/

RUN cargo build --release --bin pdf2md --bin detect-pdf --bin server

# ---- Runtime stage ----
FROM debian:bookworm-slim

RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/*

COPY --from=builder /app/target/release/pdf2md /usr/local/bin/
COPY --from=builder /app/target/release/detect-pdf /usr/local/bin/
COPY --from=builder /app/target/release/server /usr/local/bin/

COPY --from=builder /app/external/bcmaps /opt/pdf-inspector/bcmaps
ENV PDF_INSPECTOR_BCMAPS_DIR=/opt/pdf-inspector/bcmaps
ENV PORT=3000

EXPOSE 3000

ENTRYPOINT ["server"]
