FROM rust:1.99.1-slim-bookworm
RUN rustup component add rustfmt
RUN apt-get update && apt-get install -y --no-install-recommends ripgrep \
    && rm -rf /var/lib/apt/lists/*
WORKDIR /work
CMD ["cargo", "test", "--locked"]
