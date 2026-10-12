#!/usr/bin/env bash
set -euo pipefail

repository="$PWD"
: "${CARGO_TARGET_DIR:?Select an explicit compiler-floor build target}"
archives="$repository/target/package/tmp-crate"
evidence=$(mktemp -d "$repository/target/package/compiler-floor-evidence.XXXXXX")
git ls-files --cached --others --exclude-standard -z | xargs -0 sha256sum > "$evidence/source-before.sha256"
export BOMBAY_YAML_SOURCE_CORPUS="$repository/DEFAULT_CONFIG.json5"
rustc --version --verbose | tee "$evidence/rustc.txt"
cargo --version --verbose | tee "$evidence/cargo.txt"
compiler_version=$(rustc --version)
[[ "$compiler_version" == 'rustc 1.88.0 '* ]]

packages=(bombay-zenoh-config bombay-zenoh-link-tls bombay-zenoh-plugin-trait
          bombay-zenoh-link bombay-zenoh-transport bombay-zenoh)
package_arguments=()
for package in "${packages[@]}"; do
  package_arguments+=(-p "$package")
done
cargo check --locked --jobs 2 --lib "${package_arguments[@]}" \
  2>&1 | tee "$evidence/source-default.log"
for features in transport_tls transport_tls,internal transport_tls,unstable \
    transport_tls,test transport_tls,internal,unstable,test; do
  cargo check --locked --jobs 2 --lib "${package_arguments[@]}" \
    --no-default-features --features "$features" \
    2>&1 | tee "$evidence/source-$features.log"
done
for features in '' maintainer maintainer_macros; do
  cargo run --manifest-path ci/configuration-consumer/Cargo.toml --locked --jobs 2 \
    --features "$features" 2>&1 | tee "$evidence/consumer-${features:-normal}.log"
done

# Reuse the audited immutable archives; preserve prior compiler-profile logs.
mkdir -p "$evidence/zenoh" "$evidence/target/package"
cp zenoh/Cargo.toml "$evidence/zenoh/Cargo.toml"
cp target/package/audited-cohort.json "$evidence/target/package/audited-cohort.json"
(
  cd "$evidence"
  python3 "$repository/ci/verify_packaged_queries.py" \
    --source-profile configuration-cohort --archives "$archives" \
    2>&1 | tee packaged-native.log
)

git ls-files --cached --others --exclude-standard -z | xargs -0 sha256sum > "$evidence/source-after.sha256"
cmp "$evidence/source-before.sha256" "$evidence/source-after.sha256"
