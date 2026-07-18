FROM rust:1.90-bookworm AS stateful-builder
WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY crates ./crates
RUN cargo build --release -p stateful-cli

FROM python:3.14.6-slim-bookworm
ARG OMP_VERSION=17.0.4
ENV DEBIAN_FRONTEND=noninteractive \
    BUN_INSTALL=/opt/bun \
    CARGO_HOME=/usr/local/cargo \
    RUSTUP_HOME=/usr/local/rustup \
    PATH=/opt/bun/bin:/usr/local/cargo/bin:/usr/local/bin:/usr/bin:/bin \
    PYTHONDONTWRITEBYTECODE=1
RUN apt-get update \
    && apt-get install -y --no-install-recommends bash bubblewrap ca-certificates curl git build-essential less pkg-config unzip \
    && rm -rf /var/lib/apt/lists/* \
    && curl -fsSL https://bun.sh/install | bash \
    && bun install -g "@oh-my-pi/pi-coding-agent@${OMP_VERSION}" \
    && ln -s /opt/bun/bin/omp /usr/local/bin/omp
COPY --from=stateful-builder /usr/local/cargo /usr/local/cargo
COPY --from=stateful-builder /usr/local/rustup /usr/local/rustup
COPY --from=stateful-builder /src/target/release/stateful /usr/local/bin/stateful
COPY crates/stateful-bench/scripts/statefulbench_container_entry.py /usr/local/bin/statefulbench-container-entry
COPY crates/stateful-bench/scripts/statefulbench_container_diagnostics.py /usr/local/bin/statefulbench-container-diagnostics

RUN chmod 0755 /usr/local/bin/statefulbench-container-entry /usr/local/bin/statefulbench-container-diagnostics \
    && python3 --version \
    && omp --version \
    && stateful --help >/dev/null \
    && git --version \
    && rustc --version \
    && cargo --version \
    && command -v bwrap
WORKDIR /workspace
