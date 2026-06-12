# Multi-stage build for the MCSS backend.
FROM rust:1.96-slim AS builder
WORKDIR /app
COPY Cargo.toml Cargo.lock ./
COPY src ./src
COPY migrations ./migrations
RUN cargo build --release --bin mcss-backend

FROM debian:bookworm-slim
RUN useradd --system --create-home mcss \
    && mkdir -p /home/mcss/data && chown -R mcss:mcss /home/mcss/data
USER mcss
WORKDIR /home/mcss
COPY --from=builder /app/target/release/mcss-backend /usr/local/bin/mcss-backend
ENV DATABASE_URL=sqlite:///home/mcss/data/mcss.db?mode=rwc \
    UPLOAD_DIR=/home/mcss/data/uploads \
    PORT=3000
EXPOSE 3000
CMD ["mcss-backend"]
