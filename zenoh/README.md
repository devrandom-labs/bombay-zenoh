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

After verified registry publication, the selected dependency spelling is:

```toml
zenoh = { package = "bombay-zenoh", version = "=1.10.2", default-features = false, features = ["transport_tls"] }
```

Local package verification is a release rehearsal; it does not establish
registry availability. The original upstream project's website and API
references describe its wider profile: [Eclipse Zenoh](https://zenoh.io/) and
[upstream source at the selected revision](https://github.com/eclipse-zenoh/zenoh/tree/1211779c3647f5a96713dade452c546a07823580).
The controlled source and package-release history belong to
[devrandom-labs/bombay-zenoh](https://github.com/devrandom-labs/bombay-zenoh).

The proposed `1.10.2` successor also corrects resource registration for valid
leading UTF-8 path chunks. Its eight controlled dependencies retain their
published versions. Use this version only after its registry publication is
verified; source preparation and packaging checks alone do not establish it.
