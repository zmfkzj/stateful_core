FROM rust:1-bookworm AS builder
WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY crates ./crates
RUN cargo build --release -p stateful-cli

FROM debian:bookworm-slim
ENV DEBIAN_FRONTEND=noninteractive
RUN apt-get update \
    && apt-get install -y --no-install-recommends \
        bash \
        bubblewrap \
        ca-certificates \
        curl \
        git \
        python3 \
        unzip \
        xz-utils \
    && rm -rf /var/lib/apt/lists/*

RUN curl -fsSL https://bun.sh/install | bash
ENV BUN_INSTALL=/root/.bun
ENV PATH=/root/.bun/bin:/usr/local/bin:/usr/bin:/bin
RUN bun install -g @oh-my-pi/pi-coding-agent@16.1.14 \
    && ln -sf /root/.bun/bin/omp /usr/local/bin/omp \
    && omp --version

COPY --from=builder /src/target/release/stateful /usr/local/bin/stateful
RUN stateful --help >/dev/null \
    && command -v bwrap \
    && command -v omp \
    && command -v stateful
