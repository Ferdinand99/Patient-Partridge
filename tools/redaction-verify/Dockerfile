# Build and test image for the redaction verifier: the pinned Rust toolchain plus the two
# independent tools it drives (qpdf and Poppler). The base is the image of the CI runner
# (ubuntu-24.04), so the tools are the ones that CI uses: qpdf 11.9.x and Poppler 24.02.x. The
# verifier prints the versions it ran with, so a result can be tied to them.
# Build with no context:
#   docker build -t redaction-verify:1 - < tools/redaction-verify/Dockerfile
FROM ubuntu:24.04
ENV DEBIAN_FRONTEND=noninteractive \
    RUSTUP_HOME=/usr/local/rustup \
    CARGO_HOME=/usr/local/cargo \
    PATH=/usr/local/cargo/bin:$PATH
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates curl build-essential qpdf poppler-utils \
    && rm -rf /var/lib/apt/lists/* \
    && curl -fsSL https://sh.rustup.rs | sh -s -- -y --profile minimal --default-toolchain 1.99.1 \
    && rustup component add clippy rustfmt \
    && qpdf --version | head -1 && pdftotext -v 2>&1 | head -1
