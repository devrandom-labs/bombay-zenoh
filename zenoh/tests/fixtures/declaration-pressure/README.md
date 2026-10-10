# Declaration-pressure TLS fixtures

These public test credentials are exact PEM literal bytes from
`io/zenoh-transport/tests/unicast_openclose.rs::get_tls_certs` at controlled
revision `373d25a9261e4d3a6b5e2ab739142fad448c2ed0`, based on upstream Zenoh
`1211779c3647f5a96713dade452c546a07823580` (1.10.1). The source attributes their
generation to [minica](https://github.com/jsha/minica). Preserve the upstream
EPL-2.0 OR Apache-2.0 license and ZettaScale attribution in that owning source;
the repository LICENSE and NOTICE.md apply here too. No CA private key is copied.

The PEM files preserve the literals' absence of a final newline. SHA-256:

| File | SHA-256 |
| --- | --- |
| root-ca.pem | ad9c002d1d9cca03305da59045b62fba9dd03e833d330832385d94de6c8f618b |
| localhost-certificate.pem | 36c8780b6f781f7f18042989dc7d83dd234f0fd20c618f71fd875407b4bd0f58 |
| localhost-private-key.pem | b31abac5d364db6252b34af1933af6a7ecb0cc29cda8f779675438f510ed5bbb |

The CA is valid March 6, 2023 16:03:07 UTC through March 6, 2123 16:03:07 UTC;
the leaf is valid that day at 16:03:18 UTC through the same time in 2123.
The leaf permits server/client authentication and names DNS localhost. Its key
is deliberately public: never use these credentials or roots in a deployment.

The test uses separate native sessions sharing this TLS principal. It observes
routing-state growth outside the configured ACL publication keys, then exact
native undeclaration restoration. It proves neither identity isolation, enforced
resource quotas, complete memory accounting nor full callback retirement.

Run from the repository root through pinned Nix (add `--release` for optimized):

```sh
nix develop -c env BOMBAY_DECLARATION_CERTIFICATES="$PWD/zenoh/tests/fixtures/declaration-pressure" cargo test --locked --jobs 2 -p bombay-zenoh --lib --no-default-features --features transport_tls net::tests::tables::declarations_outside_acl_retain_resources -- --ignored --exact --nocapture
```

`ci/verify-release.sh` runs both profiles and strict owning Clippy. The test's
counts and waits are finite fixture inputs, not production resource defaults.
