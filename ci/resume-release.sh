#!/usr/bin/env bash
set -euo pipefail

# This reviewed automation may release only the unchanged source from the retained operation.
[[ "${GITHUB_REF:?}" == refs/heads/main ]] || { printf 'Resumption requires main\n' >&2; exit 1; }
automation="$PWD"
automation_commit=$(git rev-parse HEAD)
source_commit=e82481825e313ff14e5ea3a1d6e842040945372a
original_run=38028659029
[[ "$automation_commit" == "${GITHUB_SHA:?}" && "$source_commit" == "${EXPECTED_SOURCE_COMMIT:?}" ]] || {
  printf 'Resumption source or automation mismatch\n' >&2; exit 1;
}
require_current_automation() {
  remote_commit=$(gh api "repos/${GITHUB_REPOSITORY:?}/git/ref/heads/main" --jq '.object.sha')
  [[ "$automation_commit" == "$remote_commit" ]] || { printf 'Main changed before resumption\n' >&2; exit 1; }
}
require_verified_commit() {
  required_check=$(gh api "repos/$GITHUB_REPOSITORY/commits/$1/check-runs?per_page=100" \
    --jq '[.check_runs[] | select(.name == "Controlled TLS dependencies" and .app.id == 15368)] | sort_by(.started_at) | last | .conclusion')
  [[ "$required_check" == success ]] || { printf 'Selected revision lacks required passing CI\n' >&2; exit 1; }
}
require_current_automation
require_verified_commit "$automation_commit"
require_verified_commit "$source_commit"

retained="$automation/target/first-release"
source="$automation/target/retained-source"
mkdir -p "$retained"
gh api "repos/$GITHUB_REPOSITORY/actions/runs/$original_run" > "$retained/run.json"
artifact_name="controlled-release-$source_commit"
artifact_count=$(gh api "repos/$GITHUB_REPOSITORY/actions/runs/$original_run/artifacts" \
  --jq "[.artifacts[] | select(.name == \"$artifact_name\" and .expired == false)] | length")
[[ "$artifact_count" == 1 ]] || { printf 'Original retained artifact is unavailable or ambiguous\n' >&2; exit 1; }
gh run download "$original_run" --repo "$GITHUB_REPOSITORY" --name "$artifact_name" --dir "$retained"
python3 ci/release_cohort.py origin "$retained" "$retained/tmp-crate"
git worktree add --detach "$source" "$source_commit"
[[ -z "$(git -C "$source" status --porcelain)" ]] || { printf 'Original source is dirty\n' >&2; exit 1; }
mkdir -p "$source/target/package/tmp-crate"
cp "$retained/tmp-crate/"*.crate "$source/target/package/tmp-crate/"
python3 "$source/ci/verify_archives.py" "$source" "$retained/tmp-crate"
python3 ci/release_cohort.py before "$retained" "$source/target/package/tmp-crate"

# Only these absent packages are selected. Their ordinary dependency features reproduce
# the original nine-package archives; the credential gate requires that exact identity.
package_arguments=(
  -p bombay-zenoh-plugin-trait -p bombay-zenoh-link
  -p bombay-zenoh-transport -p bombay-zenoh
)
cd "$source"
cargo publish --dry-run "${package_arguments[@]}" --locked --jobs 2
python3 "$automation/ci/release_cohort.py" candidates "$retained" "$source/target/package/tmp-crate"
python3 ci/verify_archives.py "$source" "$source/target/package/tmp-crate" "$source/target/package/audited-cohort.json"
# Build the unchanged owning credential SDK from reviewed automation, never the retired source.
cargo build --manifest-path "$automation/ci/release-credential/Cargo.toml" --locked --jobs 2
provider="$automation/ci/release-credential/target/debug/bombay-release-credential"
cohort="$source/target/package/remaining-cohort.json"
unset CARGO_REGISTRY_TOKEN CARGO_REGISTRIES_CRATES_IO_TOKEN
require_current_automation
# Refresh receipts immediately before the first write, preserving the original five.
python3 "$automation/ci/release_cohort.py" before "$retained" "$source/target/package/tmp-crate"
publish_status=0
cargo --config "registry.credential-provider=[\"$provider\",\"$cohort\"]" \
  publish "${package_arguments[@]}" --locked --jobs 2 || publish_status=$?
receipt_status=0
python3 "$automation/ci/release_cohort.py" after "$retained" "$source/target/package/tmp-crate" || receipt_status=$?
if [[ "$publish_status" != 0 ]]; then exit "$publish_status"; fi
if [[ "$receipt_status" != 0 ]]; then exit "$receipt_status"; fi
printf 'Exact nine-package registry receipts preserved; clean registry-only consumer remains required.\n'
