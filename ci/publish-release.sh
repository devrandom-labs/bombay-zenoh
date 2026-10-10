#!/usr/bin/env bash
set -euo pipefail

# Run only through this repository's pinned Nix shell.
case "${RELEASE_OPERATION:?}" in
  verify|publish) ;;
  *) printf 'Unknown release operation\n' >&2; exit 1 ;;
esac
[[ "${GITHUB_REF:?}" == refs/heads/main ]] || { printf 'Release requires main\n' >&2; exit 1; }
source_commit=$(git rev-parse HEAD)
[[ "$source_commit" == "${EXPECTED_SOURCE_COMMIT:?}" && "$source_commit" == "${GITHUB_SHA:?}" ]] || {
  printf 'Selected source does not match the exact workflow commit\n' >&2; exit 1;
}
remote_commit=$(gh api "repos/${GITHUB_REPOSITORY:?}/git/ref/heads/main" --jq '.object.sha')
[[ "$source_commit" == "$remote_commit" ]] || { printf 'Main changed before release\n' >&2; exit 1; }
required_check=$(gh api "repos/$GITHUB_REPOSITORY/commits/$source_commit/check-runs?per_page=100" \
  --jq '[.check_runs[] | select(.name == "Controlled TLS dependencies" and .app.id == 15368)] | sort_by(.started_at) | last | .conclusion')
[[ "$required_check" == success ]] || { printf 'Exact source lacks required passing CI\n' >&2; exit 1; }
[[ -n "${CARGO_REGISTRY_TOKEN:-}" ]] || { printf 'Publishing credential is unavailable\n' >&2; exit 1; }

release_packages=(
  bombay-validated-struct-macros bombay-validated-struct
  bombay-zenoh-link-commons bombay-zenoh-config bombay-zenoh-link-tls
  bombay-zenoh-plugin-trait bombay-zenoh-link bombay-zenoh-transport bombay-zenoh
)
package_arguments=()
for package in "${release_packages[@]}"; do package_arguments+=(-p "$package"); done
cargo package "${package_arguments[@]}" --features json_get,json5 --locked --jobs 2
python3 ci/verify_archives.py "$PWD" "$PWD/target/package"
if [[ "$RELEASE_OPERATION" == verify ]]; then
  printf 'Verified exact source and archives; token publishing scope remains unproved.\n'
  exit 0
fi

# Cargo/crates.io authorize the exact package before registry writes. A dry run
# does not authenticate. Never replace this owning operation with a custom probe.
for package in "${release_packages[@]}"; do
  cargo publish --locked --jobs 2 -p "$package"
  PACKAGE_NAME="$package" SOURCE_COMMIT="$source_commit" python3 - <<'PY'
import hashlib
import json
import os
from pathlib import Path
from urllib.request import Request, urlopen

package = os.environ["PACKAGE_NAME"]
version = "2.2.0" if package.startswith("bombay-validated-struct") else "1.10.1"
archive = Path("target/package") / f"{package}-{version}.crate"
checksum = hashlib.sha256(archive.read_bytes()).hexdigest()
request = Request(f"https://crates.io/api/v1/crates/{package}/{version}",
                  headers={"User-Agent": "Bombay controlled dependency release"})
with urlopen(request, timeout=30) as response:
    published = json.load(response)["version"]
if published["num"] != version or published["checksum"] != checksum:
    raise SystemExit("Published archive does not match the exact local artifact")
receipt = f"{package} {version}: {checksum}; source {os.environ['SOURCE_COMMIT']}"
print(receipt)
with open(os.environ["GITHUB_STEP_SUMMARY"], "a") as summary:
    summary.write(receipt + "\n")
PY
done
printf 'Publication receipts preserved; clean registry-only consumer verification remains required.\n'
