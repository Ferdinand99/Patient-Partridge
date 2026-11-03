#!/usr/bin/env bash
# Builds and tests the redaction verifier on LANBox (Docker context "unraid"), never on this PC.
#
#   tools/redaction-verify/lanbox.sh                          cargo test --locked
#   tools/redaction-verify/lanbox.sh clippy --all-targets --locked -- -D warnings
#   tools/redaction-verify/lanbox.sh fmt                      formats the sources here, via LANBox
#
# The sources go in over a pipe and the build output lives in tmpfs, so nothing is written to
# the server's disks. The image (the pinned Rust toolchain plus qpdf and Poppler) is built once
# from ./Dockerfile. Containers are named partridge-verifier-<step>-<pid> and carry the labels
# owner=partridge and dev.run=partridge-verifier. The script removes nothing but its own
# --rm containers.
set -euo pipefail
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ctx="${DOCKER_CONTEXT:-unraid}"
image="${VERIFIER_IMAGE:-partridge-verifier:1}"
step="${DEV_RUN_STEP:-test}"
docker="docker --context $ctx"

if ! $docker image inspect "$image" >/dev/null 2>&1; then
  $docker build --label owner=partridge --label dev.run=partridge-verifier -t "$image" - < "$here/Dockerfile"
fi

args=("$@")
[ ${#args[@]} -eq 0 ] && args=(test --locked)

common=(--rm -i
  --label owner=partridge --label dev.run=partridge-verifier
  --cpus 2 --memory 4g
  --tmpfs /work:exec,size=3g
  -e CARGO_HOME=/work/cargo -e CARGO_TARGET_DIR=/work/target
  -e CARGO_BUILD_JOBS=2 -e RUST_TEST_THREADS=2)

if [ "${args[0]}" = "fmt" ]; then
  tar -C "$here" --exclude=target -cf - . | MSYS_NO_PATHCONV=1 $docker run "${common[@]}" \
    --name "partridge-verifier-fmt-$$" --label "dev.title=redaction verifier fmt" \
    "$image" sh -c 'mkdir -p /work/src && tar -xf - -C /work/src && cd /work/src && cargo fmt >&2 && tar -cf - src tests' \
    | tar -C "$here" -xf -
  exit 0
fi

tar -C "$here" --exclude=target -cf - . | MSYS_NO_PATHCONV=1 $docker run "${common[@]}" \
  --name "partridge-verifier-$step-$$" --label "dev.title=redaction verifier $step" \
  "$image" sh -c 'mkdir -p /work/src && tar -xf - -C /work/src && cd /work/src && cargo "$@"' sh "${args[@]}"
