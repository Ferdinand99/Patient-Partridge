#!/usr/bin/env bash
# Builds and tests the redaction verifier in a container, so that nothing heavy runs on the machine
# you work on.
#
#   tools/redaction-verify/scripts/run-in-docker.sh                       cargo test --locked
#   tools/redaction-verify/scripts/run-in-docker.sh clippy --all-targets --locked -- -D warnings
#   tools/redaction-verify/scripts/run-in-docker.sh fmt                   formats the sources here
#   tools/redaction-verify/scripts/run-in-docker.sh lock                  updates Cargo.lock here
#
# The sources go in over a pipe and the build output lives in tmpfs. The image (the pinned Rust
# toolchain plus qpdf and Poppler) is built once from ../Dockerfile.
#
# Environment:
#   DOCKER_CONTEXT      the Docker context to use (default: the current context)
#   VERIFIER_IMAGE      the image name (default: redaction-verify:1)
#   CONTAINER_PREFIX    prefix of the container names (default: redaction-verify)
#   STEP                a word for the container name (default: test)
#   RUN_LABELS          space separated key=value labels for the image build and the containers
#   CARGO_CACHE_VOLUME  a Docker volume to keep the downloaded crates in (default: tmpfs)
#
# The script removes nothing but its own --rm containers.
set -euo pipefail
here="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
image="${VERIFIER_IMAGE:-redaction-verify:1}"
prefix="${CONTAINER_PREFIX:-redaction-verify}"
step="${STEP:-test}"

docker=(docker)
[ -n "${DOCKER_CONTEXT:-}" ] && docker+=(--context "$DOCKER_CONTEXT")

labels=()
for l in ${RUN_LABELS:-}; do labels+=(--label "$l"); done

if ! "${docker[@]}" image inspect "$image" >/dev/null 2>&1; then
  "${docker[@]}" build "${labels[@]}" -t "$image" - < "$here/Dockerfile"
fi

args=("$@")
[ ${#args[@]} -eq 0 ] && args=(test --locked)

common=(--rm -i "${labels[@]}"
  --cpus 2 --memory 4g
  --tmpfs /work:exec,size=3g
  -e CARGO_HOME=/work/cargo -e CARGO_TARGET_DIR=/work/target
  -e CARGO_BUILD_JOBS=2 -e RUST_TEST_THREADS=2)
if [ -n "${CARGO_CACHE_VOLUME:-}" ]; then
  common+=(-v "$CARGO_CACHE_VOLUME:/cargo-cache" -e CARGO_HOME=/cargo-cache)
fi

if [ "${args[0]}" = "lock" ]; then
  tar -C "$here" --exclude=target -cf - . | MSYS_NO_PATHCONV=1 "${docker[@]}" run "${common[@]}" \
    --name "$prefix-lock-$$" --label "dev.title=redaction verifier lock file" \
    "$image" sh -c 'mkdir -p /work/src && tar -xf - -C /work/src && cd /work/src && cargo fetch >&2 && tar -cf - Cargo.lock' \
    | tar -C "$here" -xf -
  exit 0
fi

if [ "${args[0]}" = "fmt" ]; then
  tar -C "$here" --exclude=target -cf - . | MSYS_NO_PATHCONV=1 "${docker[@]}" run "${common[@]}" \
    --name "$prefix-fmt-$$" --label "dev.title=redaction verifier fmt" \
    "$image" sh -c 'mkdir -p /work/src && tar -xf - -C /work/src && cd /work/src && cargo fmt >&2 && tar -cf - src tests' \
    | tar -C "$here" -xf -
  exit 0
fi

tar -C "$here" --exclude=target -cf - . | MSYS_NO_PATHCONV=1 "${docker[@]}" run "${common[@]}" \
  --name "$prefix-$step-$$" --label "dev.title=redaction verifier $step" \
  "$image" sh -c 'mkdir -p /work/src && tar -xf - -C /work/src && cd /work/src && cargo "$@"' sh "${args[@]}"
