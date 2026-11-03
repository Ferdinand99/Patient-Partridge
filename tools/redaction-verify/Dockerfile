# Build and test image for the redaction verifier: the pinned Rust toolchain plus the two
# independent tools it drives (qpdf and Poppler). Build with no context:
#   docker build -t partridge-verifier:1 - < tools/redaction-verify/Dockerfile
FROM rust:1.99.1-bookworm
RUN apt-get update \
    && apt-get install -y --no-install-recommends qpdf poppler-utils \
    && rm -rf /var/lib/apt/lists/*
RUN rustup component add clippy rustfmt
