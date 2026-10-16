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

# Refuse a historical cohort requested from a different owning source before Cargo or credentials.
PYTHONDONTWRITEBYTECODE=1 python3 - "${RELEASE_KIND:?}" <<'PY_SOURCE'
import sys
import tomllib
from pathlib import Path
from ci.verify_archives import ReleaseKind, require, version
require(tomllib.loads(Path("zenoh/Cargo.toml").read_text())["package"]["version"] == version("zenoh", ReleaseKind(sys.argv[1])), "release kind and owning source version conflict")
PY_SOURCE

case "${RELEASE_KIND:?}" in
  zenoh-successor) release_packages=(bombay-zenoh) ;;
  configuration-successor)
    release_packages=(bombay-zenoh-config bombay-zenoh-link-tls
      bombay-zenoh-plugin-trait bombay-zenoh-link bombay-zenoh-transport bombay-zenoh) ;;
  tls-security-successor)
    release_packages=(bombay-zenoh-link-commons bombay-zenoh-link-tls
      bombay-zenoh-link bombay-zenoh-transport bombay-zenoh) ;;
  tls-ingress-successor)
    release_packages=(bombay-zenoh-link-tls bombay-zenoh-link bombay-zenoh-transport bombay-zenoh) ;;
  *) printf 'Unknown release kind\n' >&2; exit 1 ;;
esac
package_arguments=()
for package in "${release_packages[@]}"; do package_arguments+=(-p "$package"); done
# Cargo verifies the exact selected successor closure against immutable dependencies.
cargo publish --dry-run "${package_arguments[@]}" --no-default-features --features transport_tls --locked --jobs 2
cohort="$PWD/target/package/audited-cohort.json"
python3 ci/verify_archives.py "$PWD" "$PWD/target/package/tmp-crate" "$cohort" --release-kind "$RELEASE_KIND"
if [[ "$RELEASE_OPERATION" == verify ]]; then
  printf 'Verified exact source and publication candidates; publishing authority remains unproved.\n'
  exit 0
fi

# The owning SDK vetoes a changed retained tarball before releasing an operation-bound token.
cargo build --manifest-path ci/release-credential/Cargo.toml --locked --jobs 2
provider="$PWD/ci/release-credential/target/debug/bombay-release-credential"
unset CARGO_REGISTRY_TOKEN CARGO_REGISTRIES_CRATES_IO_TOKEN
record_registry_receipts() {
  RECEIPT_PHASE="$1" SOURCE_COMMIT="$source_commit" python3 - "$cohort" <<'PY_RECEIPTS'
import json
import os
import sys
from pathlib import Path
from urllib.error import HTTPError, URLError
from urllib.request import Request, urlopen

cohort = json.loads(Path(sys.argv[1]).read_text())
phase = os.environ["RECEIPT_PHASE"]
path = Path("target/package/registry-receipts.json")
receipts = json.loads(path.read_text()) if path.exists() else {"source": os.environ["SOURCE_COMMIT"]}
receipts[phase] = {}
refused = []
for package, archive in cohort.items():
    request = Request(f"https://crates.io/api/v1/crates/{package}/{archive['version']}",
                      headers={"User-Agent": "Bombay controlled dependency release"})
    try:
        with urlopen(request, timeout=30) as response:
            published = json.load(response)["version"]
        receipt = {"version": published["num"], "checksum": published["checksum"]}
        if receipt != archive:
            refused.append(f"{package}: registered archive differs")
        elif phase == "before":
            refused.append(f"{package}: already registered; prepare a reviewed successor version")
    except HTTPError as error:
        receipt = {"status": "absent" if error.code == 404 else "unavailable", "http": error.code}
        if error.code != 404 or phase == "after":
            refused.append(f"{package}: publication not established")
    except (URLError, TimeoutError, KeyError, ValueError):
        receipt = {"status": "unavailable"}
        refused.append(f"{package}: registry receipt unavailable")
    receipts[phase][package] = receipt
    path.write_text(json.dumps(receipts, indent=2, sort_keys=True) + "\n")
    summary = f"{package} {archive['version']}: {json.dumps(receipt, sort_keys=True)}; source {receipts['source']}"
    print(summary)
    with open(os.environ["GITHUB_STEP_SUMMARY"], "a") as recorded:
        recorded.write(summary + "\n")
if refused:
    raise SystemExit("\n".join(refused))
PY_RECEIPTS
}
record_registry_receipts before
remote_commit=$(gh api "repos/$GITHUB_REPOSITORY/git/ref/heads/main" --jq '.object.sha')
[[ "$source_commit" == "$remote_commit" ]] || { printf 'Main changed before publication\n' >&2; exit 1; }
publish_status=0
cargo --config "registry.credential-provider=[\"$provider\",\"$cohort\"]" \
  publish "${package_arguments[@]}" --no-default-features --features transport_tls --locked --jobs 2 || publish_status=$?
receipt_status=0
record_registry_receipts after || receipt_status=$?
# Preserve an actual publication refusal even when the already uploaded prefix verifies.
if [[ "$publish_status" != 0 ]]; then exit "$publish_status"; fi
if [[ "$receipt_status" != 0 ]]; then exit "$receipt_status"; fi
printf 'Publication receipts preserved; clean registry-only consumer verification remains required.\n'
