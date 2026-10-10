# Configuration dependency source custody

These Bombay-maintained copies retain upstream 2.2.0 under distinct package names.
UPSTREAM_CARGO.toml and UPSTREAM_VCS.json describe the upstream input, not the corrected output.
Cargo reserves Cargo.toml.orig for its own generated archive metadata; upstream
manifest bytes are preserved under the explicit custody filename instead.
The upstream archives declare EPL-2.0 OR Apache-2.0 but contain no license file.
Each copy includes the matching dual-license text from the selected Zenoh source
(`LICENSE` SHA-256 `01a44774f7b1a453595c7c6d7f7308284ba6a1059dc49e14dad6647e1d44a338`).

| Package | Upstream source commit | Original manifest SHA-256 |
| --- | --- | --- |
| bombay-validated-struct | `9cd5eda2d2a90e84cf47f3e0cdfb21e1df9d6729` | `4815436fc189203379cc79af631f37125c9e9fdc4a142187b6fdb8d5af392e7e` |
| bombay-validated-struct-macros | `3eebb3c2e9147ac6c7c8cde987d89b22c9ab098a` | `05375481cb7d3308f010aa8c18ea627306569bde9593cada1c282023e8286746` |

Original registry archive checksums, independently matched to the selected lock:

- `validated_struct` 2.2.0: `869a93e8a7286e339e1128630051d82babbcd75d585975af07b9f3327220e60e`.
- `validated_struct_macros` 2.2.0: `8c44ce98e7227a04eeb4cf9c784109a5c9710e54849ceb4f09f8597247897f1e`.

Corrections authorized by Bombay’s static remote actor PRD: safe owned field movement,
equivalent constructor shorthand, generated import conformance and preserved Rust library aliases.
No upstream source identity, arbitrary consumer alias support or registry availability is claimed.
