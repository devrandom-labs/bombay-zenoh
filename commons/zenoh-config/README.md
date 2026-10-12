# Bombay-maintained `bombay-zenoh-config`

This crate owns configuration for Bombay's controlled Zenoh copy.
Its internal API may change in any version, including patch updates.

After verified publication, applications should depend on `bombay-zenoh` (Rust alias `zenoh`) and use its supported public APIs.

- [Supported TLS-only profile and installation](https://github.com/devrandom-labs/bombay-zenoh/blob/main/zenoh/README.md)
- [Original upstream source](https://github.com/eclipse-zenoh/zenoh/tree/1211779c3647f5a96713dade452c546a07823580)
- Upstream attribution and the `EPL-2.0 OR Apache-2.0` license are preserved; exact source provenance is recorded in `package.metadata.bombay`.

Candidate `1.10.2` adds optional `queries_max_retained: Option<NonZeroUsize>`.
The generated getter, validated setter and configuration key share this owner;
the additional field changes the generated internal `Config::new` arity.
Use the top-level Config loading/insertion interface in ordinary applications.
Limits above Tokio Semaphore::MAX_PERMITS are refused, and zero fails decoding.
Omission retains unbounded compatibility. A Session fixes its configured limit
at creation and independently checks direct deserialization before allocation.
This support successor is not published merely because local tests pass.
The limit covers ordinary QueryState registrations, excluding liveliness queries.
It declares Rust `1.88.0`. YAML loading uses maintained `yaml_serde` through
the existing `serde_yaml` alias; owning compatibility passes both source build
profiles, with normalized archive and minimum-compiler gates retained.
