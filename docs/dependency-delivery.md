# Controlled dependency delivery

This repository maintains Bombay's verified TLS-only Zenoh dependency copies.
The unmodified source base is upstream Zenoh 1.10.1, commit
`1211779c3647f5a96713dade452c546a07823580`. Its complete Git ancestry is retained.
The configuration dependency source commits, original manifests and archive
checksums are recorded in [configuration source custody](../dependencies/configuration-source.md).
Original copyright notices and dual-license texts remain unchanged, including
inherited whitespace in the copied license files.

The controlled correction retains the existing certificate verifier, with an
explicit option to exclude public roots and a private deterministic root-store
inspection seam. The configuration generator moves nested fields with standard
Rust and Syn APIs instead of its original unsafe replacement; its existing
initializer now uses field shorthand. Standard Cargo aliases retain the original
Rust library names. No replacement verifier, codec, actor model or runtime was
introduced. Supported Cargo options and the application deployment obligations
are described in the [supported profile](../zenoh/README.md).

## Verification contract

The stable compiler and Nix inputs match Bombay's pinned verification environment:
Rust/Cargo 1.99.0 and the unchanged upstream Bombay `flake.lock`. The dedicated
Miri shell uses the same pinned nightly 2026-06-15 and manifest checksum.
Run Rust commands through these shells:

```sh
nix develop -c bash ci/verify-release.sh
nix develop .#miri -c cargo miri test --locked -p bombay-validated-struct-macros --test structure
```

The required GitHub check is **Controlled TLS dependencies**. It exercises:

- Seven deterministic certificate-root selection tests in debug and optimized builds.
- The actual configured-only mutual-TLS owning connection/retirement test in both builds.
- The generator's real shared syntax implementation and its Miri integration target.
- Strict library checks for all nine owners, the retained maintainer feature combinations,
  the affected transport test, and a separate public consumer in both builds.
- Actual verified archives for all nine packages, including a source/license/provenance
  auditor with intended dependency-identity and excluded-feature inversions.

The independent consumer has its own locked workspace and uses ordinary Rust
library aliases. Relative source paths permit CI compilation before publication;
this consumer is not evidence of a successful crates.io installation. Cargo's
package verification tests the complete unpublished cohort through its temporary
registry. The resulting archives have only registry dependency requirements.

These checks certify the bounded corrections and selected dependency profile.
They do not certify every inherited upstream API or execute Bombay's remote-actor
acceptance suite. In particular, actual actor integration, Linux subprocess
campaigns, application identity, authorization, replay protection and resource
bounds remain governed by Bombay's static remote actor PRD. The inherited
nested-runtime panic test is not accepted as a client-certificate rejection witness.
The separate corrected-graph healthy/refusal campaign remains recorded in that PRD.

## Review, merge and publication

The controlled repository starts `main` at the exact upstream base. Corrections
and this repository's own verification infrastructure are separately reviewable
commits in a pull request. Replace the upstream workflow suite rather than run
its unsupported transport matrix, official-package release jobs, mutable tooling
or Eclipse release services. Require **Controlled TLS dependencies** before merge,
retain independent correction review, and record the actual PR, successful check
and merge commit. A local package build or open PR does not establish delivery.

Publication is manual after review and passing required CI. Authenticate using
publishing authority scoped to these packages; no registry credentials are stored
in this source repository. First confirm that the selected package versions remain
available. Publish in dependency order, allowing the registry to expose each
version before publishing a dependent:

1. `bombay-validated-struct-macros` 2.2.0.
2. `bombay-validated-struct` 2.2.0 and `bombay-zenoh-link-commons` 1.10.1.
3. `bombay-zenoh-config` 1.10.1.
4. `bombay-zenoh-link-tls` and `bombay-zenoh-plugin-trait` 1.10.1.
5. `bombay-zenoh-link` 1.10.1.
6. `bombay-zenoh-transport` 1.10.1.
7. `bombay-zenoh` 1.10.1.

The manual workflow prepares all nine publication candidates with Cargo's same
selected `json_get,json5` feature graph. It audits their exact source revision,
normalized manifests and all 29 corrected archived lock checksums. Publication
uses one standard Cargo multi-package invocation; Cargo owns dependency ordering
and registry visibility waits. A private credential executable uses the owning
`cargo-credential` 0.4.11 SDK. Its registry-specific configuration excludes other
credential providers, and each publication receives an uncached credential only
after the actual retained tarball digest matches the audited cohort. The SDK owns
all credential IPC; this executable defines no upload or authentication format.

Do not bypass package verification or publish from an unreviewed revision.
Record the published archive checksum and source commit for every package.
Then verify a clean application installation from crates.io with no path, Git,
workspace or `[patch]` overrides, including the supported aliases, source-custody
assertions and selected feature denials. Package availability alone does not prove
that a normal Bombay installation uses the corrected graph.

At preparation on 2026-10-10, all nine package identities returned crates.io 404.
The authenticated GitHub user can administer the organization. No local publishing
credential is configured, but the existing organization-wide
`CARGO_REGISTRY_SECRET` is available to the separately reviewed manual release
workflow. Its presence does not prove its publishing scope. The `verify` operation
checks exact current main, required successful CI and actual package archives;
it does not claim to authenticate the token. Ordinary Cargo/crates.io verifies
actual authority for the exact package before registry writes during the explicitly
selected `publish` operation. Authentication or scope refusal stops subsequent
publication without bypassing the owning registry checks. Before and after registry
receipts retain every established uploaded prefix, including a later refusal.
Cargo rejects already registered selected versions rather than silently skipping
them. A partial-cohort retry therefore needs a separately prepared, reviewed
selection preserving the original audited artifacts; the release is not atomic.
No registry publication, release tag or completed feature delivery is recorded here.
