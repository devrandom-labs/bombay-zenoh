use cargo_credential::{
    Action, CacheControl, Credential, CredentialResponse, Error, Operation, RegistryInfo, Secret,
};
use serde::Deserialize;
use std::{collections::BTreeMap, env, fs};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ArchiveIdentity {
    version: String,
    checksum: String,
}

struct ReleaseCredential;

impl Credential for ReleaseCredential {
    fn perform(
        &self,
        registry: &RegistryInfo<'_>,
        action: &Action<'_>,
        args: &[&str],
    ) -> Result<CredentialResponse, Error> {
        if registry.index_url != "https://github.com/rust-lang/crates.io-index" || args.len() != 1 {
            return Err("request is outside the audited release".into());
        }
        match action {
            Action::Get(Operation::Read) => {}
            Action::Get(Operation::Publish { name, vers, cksum }) => {
                let encoded = fs::read(args[0]).map_err(|error| Error::from(Box::new(error)))?;
                let archives: BTreeMap<String, ArchiveIdentity> = serde_json::from_slice(&encoded)
                    .map_err(|error| Error::from(Box::new(error)))?;
                match archives.get(*name) {
                    Some(archive) if archive.version == *vers && archive.checksum == *cksum => {}
                    _ => return Err("archive is outside the audited release".into()),
                }
            }
            _ => return Err("operation is outside the audited release".into()),
        }
        let token = Secret::from(
            env::var("BOMBAY_RELEASE_TOKEN")
                .map_err(|_| Error::from("publishing credential is unavailable"))?,
        );
        if token.is_empty() {
            return Err("publishing credential is unavailable".into());
        }
        Ok(CredentialResponse::Get {
            token,
            cache: CacheControl::Never,
            // The owning SDK requires this flag; no response authorizes another operation.
            operation_independent: false,
        })
    }
}

fn main() {
    cargo_credential::main(ReleaseCredential);
}
