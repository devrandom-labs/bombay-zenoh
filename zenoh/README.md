# Bombay-maintained Zenoh transport dependency

This controlled copy is based on Eclipse Zenoh 1.10.1, upstream revision
`1211779c3647f5a96713dade452c546a07823580`. It preserves the original Rust library
name `zenoh`, upstream attribution and the `EPL-2.0 OR Apache-2.0` license.
Exact provenance is included in the package's `package.metadata.bombay` fields.

The supported runtime profile is TLS only, enabled by default. Core
publish/subscribe, query/reply, key expressions, ordinary logging and the
existing session API remain available. Other transports, SHM, compression,
multiple links, separate password/RSA authentication, plugins, statistics and
additional task tracing are excluded Cargo options.

The existing `internal`, `internal_config`, `unstable` and `test` flags are
retained maintainer interfaces. Cargo consumers can enable them, but they are
unsupported for ordinary Bombay applications.

The corrected outgoing TLS client supports explicit deployment-root-only trust
selection. Bombay's approved deployment requires mutual TLS, configured deployment
roots with public-root fallback disabled, and explicit TLS connection/listen
addresses with multicast and gossip discovery disabled. Enabling the transport
feature alone does not configure those deployment requirements. Actor admission
and identity policies belong to the Bombay integration.

The currently verified published dependency spelling is:

```toml
zenoh = { package = "bombay-zenoh", version = "=1.10.8", default-features = false, features = ["transport_tls"] }
```

Local package verification is a release rehearsal; it does not establish
registry availability. The original upstream project's website and API
references describe its wider profile: [Eclipse Zenoh](https://zenoh.io/) and
[upstream source at the selected revision](https://github.com/eclipse-zenoh/zenoh/tree/1211779c3647f5a96713dade452c546a07823580).
The controlled source and package-release history belong to
[devrandom-labs/bombay-zenoh](https://github.com/devrandom-labs/bombay-zenoh).

The published `1.10.8` graph still declares Rustls `0.23.45` as its minimum.
Applications must select fixed Rustls `0.23.46` or newer in a verified lock.
Candidate `1.10.9` raises both owning TLS dependency minima to `0.23.46` without
new SDK Rust code, features or public APIs. Its exact five-package successor
includes link commons `1.10.2` and TLS link/link/transport `1.10.3`; Config/plugin
`1.10.2` and both validated-struct `2.2.0` archives remain unchanged.
The five candidates declare Rust `1.88.0`; actual source/archive/floor checks,
required CI, reviewed merge, publication and registry-only consumption remain
gates. Preparing candidate source does not establish registry availability.

Published `1.10.2` corrects resource registration for valid leading UTF-8 path
chunks. Published `1.10.3` retires finalized/disconnected native interest resources
after their last owner releases them; live shared owners retain those resources.
Published `1.10.4` preserves accepted query consumers at session identifier
exhaustion; `1.10.5` verifies that owning regression from the packaged source
with default TLS alone. Published `1.10.6` retires completed query timeout owners.

Published `1.10.7` additionally issues each routed query identifier at most
once during its existing face lifetime. Exhaustion preserves earlier consumers
and omits the exhausted route; BestMatching retains its first complete target
without failover. Native query FINAL is stream completion, not protected actor
refusal or proof of nonexecution. The eight controlled dependencies stay at their
existing published versions. Its required review/CI, registry publication and clean consumer verification
have passed. These corrections do not establish global
memory/task quotas, remote processing, router recovery or rollback after unwinds.

Published `1.10.8` uses configuration and four dependent support successors at
`1.10.2`; the other three support archives remain immutable. It adds the explicit
optional `queries_max_retained` setting for each Session. A nonzero value bounds
ordinary QueryState registrations until map state, timeout captures and active
native reply callbacks have all retired.
Separate liveliness queries are outside this count and remain unbounded.
Omission preserves unbounded compatibility; it provides no bounded-deployment
proof. The limit is fixed at Session creation, checked against Tokio's maximum
before allocation, and admission refuses immediately when capacity is unavailable.
This is a retained-query count, not a byte, executor-allocation, router-global or
whole-memory bound. This count does not establish the broader remote-service resource requirements.
