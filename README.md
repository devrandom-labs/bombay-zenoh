# Bombay controlled Zenoh dependency copy

The first registry publication retained source `e82481825e313ff14e5ea3a1d6e842040945372a`
and artifact run `38028659029`. Five packages were published with matching archive
checksums before crates.io refused the next new crate with HTTP 429.
The historical manual `resume` operation released only the four absent packages from that
unchanged source. Reviewed current-main automation and original-source CI must
both pass; the original artifact, published prefix and regenerated candidate bytes
must match. Its credential authority covers only those four exact archives.
Any further partial refusal stops the operation and preserves registry receipts;
this is neither an atomic cohort release nor an automatic retry. Registry-only
consumer verification passed after all nine publication receipts were established.

Published `bombay-zenoh 1.10.2` corrects leading UTF-8 resource chunks.
Published `1.10.3` also retires finalized/disconnected native interest resources
after their last owner releases them. Their actual registry archives and clean
registry-only consumers were verified. These corrections do not establish a
global memory quota or unwind rollback.

Published `1.10.4` refuses query-identifier exhaustion in the existing
Session before registering, retaining or dispatching a new query. IDs never wrap
or become reusable in that Session; the maximum counter value denotes exhaustion.
The last usable ID remains available. Owning tests retain real native queries,
replies and error results through actual undeclaration and session close, and
prove repeated exhaustion cannot replace another query's consumer. The existing
get error does not return its original payload. Router allocation and native
timeout-task/resource bounds remain separate open requirements.

The historical top-only successor operations retained all eight published
dependency versions and archive checksums. They audited one new owning archive
and those exact published dependencies; historical resume keeps its original source/transaction.
Required review/CI, registry publication and a clean registry-only consumer must
pass before claiming a successor delivered.

Published `1.10.5` corrected the owning query test to use its
configuration owner directly. The original published1.10.4 test relied on an
unstable wrapper interface enabled implicitly by workspace tests; its standalone
TLS-only compilation failed. The production query-ID correction and verified
1.10.4 registry-only consumer remain valid. Release CI now additionally verifies
the actual packaged owning test outside the workspace with only default TLS,
in both debug and optimized profiles. All eight support copies remain unchanged.
Required review/CI, publication, actual registry archives and clean consumer
verification passed for this release.

Published `1.10.7` preserves routed query consumers at face identifier exhaustion;
required CI, archive custody, actual publication and registry-only consumption
have passed. The separately published `1.10.8` configuration cohort uses Config,
TLS link, link, plugin trait and transport `1.10.2`; link commons `1.10.1` and
both validated-struct `2.2.0` archives remain unchanged. Its local retained-query
count does not establish router-global, handshake, byte or executor bounds.

Candidate `1.10.9` corrects the two owning Rustls minima to `0.23.46` following
RUSTSEC-2026-0346 through RUSTSEC-2026-0349. Its exact security cohort is link
commons `1.10.2`, TLS link/link/transport `1.10.3`, and top `1.10.9`. Config/plugin
`1.10.2` and both generator archives stay immutable. Current applications must
select fixed Rustls in their verified lock; older SDK declarations still permit
`0.23.45`. Local lock correction does not prove this successor published.

The candidate also corrects the TLS link's cross-direction stream ownership.
One Tokio split owns the stream, with shared exclusion during each poll and
separate ordering of read and write operations. Pending reads therefore permit
writes without simultaneous mutable access to the TLS connection. This removes
the prior UnsafeCell, manual Send/Sync implementations and blocking destructor.
Explicit close uses the owning TLS shutdown, including close_notify; abandoning
the whole owner releases its socket and does not claim graceful shutdown.
The existing Zenoh error source form retains original I/O causes. Private tests
use package-local copies of the existing validated TLS fixture certificates.
No transport, public type, dependency identity or resource default is added.
These candidate changes require their own affected-source release/CI review;
earlier checks on23676e6 do not establish delivery of the ownership correction.

All five security candidates declare Rust `1.88.0`. Their actual source,
normalized archives, compiler-floor checks, required CI, reviewed merge,
publication and registry-only consumers remain delivery gates. No numeric default
or public API change is selected by this dependency correction.

Maintainers run both pinned profiles:

```sh
nix develop -c bash ci/verify-release.sh
CARGO_TARGET_DIR="$PWD/target/compiler-floor" nix develop .#compiler-floor -c bash ci/compiler-floor.sh
```

This local source is based on upstream revision
`1211779c3647f5a96713dade452c546a07823580` (1.10.1). The controlled package
identities and exact base revisions are recorded in their Cargo metadata;
configuration-generator source custody is recorded in
[dependencies/configuration-source.md](dependencies/configuration-source.md).
The packages preserve their original Rust library names and upstream licenses.

The selected published profile supports TLS only, enabled by default. Other
transports, SHM, compression, multiple links, separate password/RSA authentication,
plugins, statistics and additional task tracing are excluded Cargo options.
Retained `internal`, `internal_config`, `unstable` and `test` flags are maintainer
interfaces, available to Cargo consumers but unsupported for ordinary applications.
The corrected TLS client can explicitly exclude public certificate roots;
Bombay additionally requires mutual TLS, configured deployment roots and explicit
addresses. Those deployment requirements are not implied by enabling TLS alone.

Current supported-profile guidance is in `zenoh/README.md`.
Internal-owner READMEs identify their controlled package and link this profile.

The upstream overview and links below describe the original project, whose
full runtime profile and release status do not describe these controlled copies.
This local source and its package rehearsals do not establish registry availability.

<img src="https://raw.githubusercontent.com/eclipse-zenoh/zenoh/master/zenoh-dragon.png" height="150">

[![CI](https://github.com/eclipse-zenoh/zenoh/actions/workflows/ci.yml/badge.svg?branch=main)](https://github.com/eclipse-zenoh/zenoh/actions?query=workflow%3ACI+branch%3Amain++)
[![Documentation Status](https://readthedocs.org/projects/zenoh-rust/badge/?version=latest)](https://zenoh-rust.readthedocs.io/en/latest/?badge=latest)
[![codecov](https://codecov.io/github/eclipse-zenoh/zenoh/branch/main/graph/badge.svg?token=F8T4C8WPZD)](https://codecov.io/github/eclipse-zenoh/zenoh)
[![Discussion](https://img.shields.io/badge/discussion-on%20github-blue)](https://github.com/eclipse-zenoh/roadmap/discussions)
[![Discord](https://img.shields.io/badge/chat-on%20discord-blue)](https://discord.gg/2GJ958VuHs)
[![License](https://img.shields.io/badge/License-EPL%202.0-blue)](https://choosealicense.com/licenses/epl-2.0/)
[![License](https://img.shields.io/badge/License-Apache%202.0-blue.svg)](https://opensource.org/licenses/Apache-2.0)

# Eclipse Zenoh

Eclipse Zenoh: Zero Overhead Pub/Sub, Store/Query and Compute.

Zenoh (pronounced _/zeno/_) unifies data in motion, data at rest, and computations. It carefully blends traditional pub/sub with geo-distributed storage, queries, and computations, while retaining a level of time and space efficiency that is well beyond any of the mainstream stacks.

Check the website [zenoh.io](http://zenoh.io) for more information and [installation instructions](https://zenoh.io/docs/getting-started/installation/).

See also the [roadmap](https://github.com/eclipse-zenoh/roadmap) for more detailed technical information.

# Structure of the Repository

This repository contains the following elements:

* [zenoh](zenoh) Rust crate

  This crate is the primary and reference implementation of the Zenoh protocol. The Zenoh libraries for other languages
  are bindings to this Rust implementation, except for the pure-C
  [zenoh-pico](https://github.com/eclipse-zenoh/zenoh-pico) (see the "Language Support" section below).

* [zenoh-ext](zenoh-ext) Rust crate

  This crate contains extended components of Zenoh:
  * `AdvancedPublisher` / `AdvancedSubscriber` - APIs for sending/receiving data with advanced delivery guarantees.
  * Data serialization support. This serialization is lightweight and universal for all `zenoh` bindings, which simplifies interoperability.

* [zenohd](zenohd) router binary

  The Zenoh router is a standalone daemon used to support Zenoh network infrastructure.

* [plugins](plugins)

  The crates related to plugin support in `zenohd`.

* [commons](commons)

  The internal crates used by `zenoh`. These crates are not intended to be imported directly, and their public APIs can be changed at any time.
  Stable APIs are provided by `zenoh` and `zenoh-ext` only.

* [examples](examples)

  Zenoh usage examples. These examples have a double purpose: they not only demonstrate writing Zenoh applications in Rust but also serve as a set of tools for experimenting with and testing Zenoh functionality.

# Documentation

* [Docs.rs for Zenoh](https://docs.rs/zenoh/latest/zenoh/)

* [Docs.rs for Zenoh-ext](https://docs.rs/zenoh-ext/latest/zenoh_ext/)

# Build and run

Install [Cargo and Rust](https://doc.rust-lang.org/cargo/getting-started/installation.html).
If you already have the Rust toolchain installed, make sure it is up to date with:

```bash
rustup update
```

Zenoh can be successfully compiled with Rust stable (>= 1.75.0), but some of its dependencies may require
newer Rust versions. The `zenoh` crate itself doesn't lock its dependencies with "=" to avoid conflicts.
Instead, we provide the [zenoh-pinned-deps-1-75](commons/zenoh-pinned-deps-1-75) crate
with `zenoh` dependencies locked to Rust 1.75-compatible versions.

To build Zenoh, simply type the command below after having followed the previous instructions:

```bash
cargo build --release --all-targets
```

There are multiple features in `zenoh`; see the full list and descriptions on [docs.rs](https://docs.rs/zenoh/latest/zenoh/). For example, to
use shared memory, it must be explicitly enabled:

```toml
zenoh = {version = "1.5.1", features = ["shared-memory"]}
```

## Examples

[Examples](examples) can be executed with Cargo, or directly from `target/release/examples`. When running with Cargo, use `--` to pass command line arguments to the examples:

### Publish/Subscribe

```bash
cargo run --example z_sub
```

```bash
cargo run --example z_pub
```

### Query/Reply

```bash
cargo run --example z_queryable
```

```bash
cargo run --example z_get
```

## Zenohd Router and Plugins

The [zenohd](zenohd) router can be run with the command `cargo run` or from `target/release/zenohd`. When running with Cargo, use `--` to pass command line arguments to `zenohd`:

```bash
cargo run -- --config DEFAULT_CONFIG.json5
```

The router's purpose is to support Zenoh network infrastructure and provide additional services using [plugins](plugins).
See more details and a directory of available plugins in the [zenohd](zenohd) readme.

# Language Support

* **Rust** - this repository
* **C** - there are two implementations with the same API:
  * [zenoh-c](https://github.com/eclipse-zenoh/zenoh-c) - Rust library binding
  * [zenoh-pico](https://github.com/eclipse-zenoh/zenoh-pico) - pure C implementation
* **C++** - [zenoh-cpp](https://github.com/eclipse-zenoh/zenoh-cpp) - C++ wrapper over C libraries
* **Python** - [zenoh-python](https://github.com/eclipse-zenoh/zenoh-python)
* **Kotlin** - [zenoh-kotlin](https://github.com/eclipse-zenoh/zenoh-kotlin)
* **Java** - [zenoh-java](https://github.com/eclipse-zenoh/zenoh-java)
* **TypeScript** - [zenoh-ts](https://github.com/eclipse-zenoh/zenoh-ts) - WebSocket client for the plugin in [zenohd](zenohd)
* **Go** - [zenoh-go](https://github.com/eclipse-zenoh/zenoh-go)

# Troubleshooting

In case of trouble, please first check [this page](https://zenoh.io/docs/getting-started/troubleshooting/) to see if the issue and its cause are already known.
Otherwise, you can ask a question on the [Zenoh Discord server](https://discord.gg/vSDSpqnbkm), or [create an issue](https://github.com/eclipse-zenoh/zenoh/issues).


Published `1.10.6` retires each query's existing timeout
owner when its native QueryState is destroyed. It uses Tokio-util's existing
cancellation guard; the Session TaskController retains actual join authority.
Native controls cover completed queries and querier retirement while a Query
remains held, with original replies, finals, undeclarations and Session.close
acquired before assertions. Release CI executes these two owning tests and the
identifier test from the audited normalized archive in both profiles with only
default TLS features. This proves prompt retained-reference retirement in local
native queries; router pressure, query quotas and blocked callbacks remain open.


The top-only `1.10.7` candidate also prevents forwarded query identifiers from
wrapping or being reused during a Face lifetime. It issues the last identifier
once and omits further directions on that exhausted face without replacing an
older query or its timeout token. Existing target selection remains intact:
All/AllComplete preserve other eligible faces, while BestMatching does not add
failover after its selected face is exhausted. Native empty-route FINAL is
query-stream completion, not authenticated actor refusal or nonexecution proof.
Owning tests retain original consumers and exact reply destinations, observe
retirement before duplicate FINAL, and join controllers/Runtime before oracles.
Release verification retains the three default-TLS packaged laws and adds two
exact router laws in a separate TLS,test maintainer profile, without workspace
feature unification. Required review/CI, actual publication and registry-only
verification remain gates. Router pressure, global quotas, actual timeout-expiry
conservation and blocked callbacks remain separate obligations.
