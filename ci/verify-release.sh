#!/usr/bin/env bash
set -euo pipefail

PYTHONDONTWRITEBYTECODE=1 python3 ci/release_cohort_tests.py

# This entrypoint must run through the repository's pinned Nix development shell.
release_packages=(
  bombay-validated-struct-macros bombay-validated-struct
  bombay-zenoh-link-commons bombay-zenoh-config bombay-zenoh-link-tls
  bombay-zenoh-plugin-trait bombay-zenoh-link bombay-zenoh-transport bombay-zenoh
)
package_arguments=()
for package in "${release_packages[@]}"; do
  package_arguments+=(-p "$package")
done

cargo fmt "${package_arguments[@]}" -- --check
cargo fmt --manifest-path ci/release-credential/Cargo.toml -- --check
for profile in debug release; do
  build_arguments=()
  if [[ "$profile" == release ]]; then
    build_arguments+=(--release)
  fi
  cargo test --manifest-path ci/release-credential/Cargo.toml --locked --jobs 2 "${build_arguments[@]}"
  cargo clippy --manifest-path ci/release-credential/Cargo.toml --locked --jobs 2 \
    "${build_arguments[@]}" --all-targets -- -D warnings
  cargo test --locked --jobs 2 "${build_arguments[@]}" \
    -p bombay-zenoh-link-tls outgoing_tls_trust_tests -- --test-threads=1
  cargo test --locked --jobs 2 "${build_arguments[@]}" \
    -p bombay-validated-struct-macros --lib --test structure
  cargo test --locked --jobs 2 "${build_arguments[@]}" \
    -p bombay-zenoh-transport --test unicast_openclose \
    --no-default-features --features transport_tls,test \
    openclose_tls_only_with_mtls_and_no_public_pki -- --exact
  cargo clippy --locked --jobs 2 "${build_arguments[@]}" \
    -p bombay-zenoh-transport --test unicast_openclose \
    --no-default-features --features transport_tls,test -- -D warnings
  BOMBAY_DECLARATION_CERTIFICATES="$PWD/zenoh/tests/fixtures/declaration-pressure" \
    cargo test --locked --jobs 2 "${build_arguments[@]}" -p bombay-zenoh --lib \
      --no-default-features --features transport_tls \
      net::tests::tables::declarations_outside_acl_retain_resources -- --ignored --exact --nocapture
  cargo test --locked --jobs 2 "${build_arguments[@]}" -p bombay-zenoh --lib \
    --no-default-features --features transport_tls \
    api::session::query_identifiers::accepted_queries_keep_their_reply_consumer_at_identifier_exhaustion -- --exact
  cargo test --locked --jobs 2 "${build_arguments[@]}" -p bombay-zenoh --lib \
    --no-default-features --features transport_tls \
    net::tests::tables::leading_utf8_chunks_preserve_registered_resources -- --exact
  cargo test --locked --jobs 2 "${build_arguments[@]}" -p bombay-zenoh --lib \
    --no-default-features --features transport_tls,test \
    net::tests::regions::interest -- --test-threads=1
  cargo clippy --locked --jobs 2 "${build_arguments[@]}" -p bombay-zenoh --lib --tests \
    --no-default-features --features transport_tls,test -- -D warnings
  cargo clippy --locked --jobs 2 "${build_arguments[@]}" \
    -p bombay-zenoh --lib --tests --no-default-features --features transport_tls -- -D warnings
  cargo clippy --locked --jobs 2 "${build_arguments[@]}" \
    "${package_arguments[@]}" --lib --features json_get,json5 -- -D warnings
  for features in transport_tls transport_tls,internal transport_tls,unstable \
      transport_tls,test transport_tls,internal,unstable,test; do
    cargo clippy --locked --jobs 2 "${build_arguments[@]}" \
      -p bombay-zenoh --lib --no-default-features --features "$features" -- -D warnings
  done
  for features in '' maintainer maintainer_macros; do
    cargo clippy --manifest-path ci/configuration-consumer/Cargo.toml \
      --locked --jobs 2 "${build_arguments[@]}" --all-targets --features "$features" -- -D warnings
    cargo run --manifest-path ci/configuration-consumer/Cargo.toml \
      --locked --jobs 2 "${build_arguments[@]}" --features "$features"
  done
done

# Verify only the successor against unchanged actual published dependencies.
cargo package -p bombay-zenoh --no-default-features --features transport_tls --locked --jobs 2
python3 ci/verify_archives.py "$PWD" "$PWD/target/package" --release-kind zenoh-successor
