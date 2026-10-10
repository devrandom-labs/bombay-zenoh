use cargo_credential::{
    Action, CacheControl, CredentialHello, CredentialRequest, CredentialResponse, Error, Operation,
    RegistryInfo,
};
use std::{
    env, fs,
    io::Write,
    process::{Command, Stdio},
    sync::atomic::{AtomicUsize, Ordering},
};

const CRATE_NAME: &str = "bombay-zenoh";
const VERSION: &str = "1.10.1";
const CHECKSUM: &str = "audited-archive-checksum";
const INDEX: &str = "https://github.com/rust-lang/crates.io-index";
static COHORT_SEQUENCE: AtomicUsize = AtomicUsize::new(0);

fn credential_request<'a>(index: &'a str, action: Action<'a>) -> CredentialRequest<'a> {
    CredentialRequest {
        v: 1,
        registry: RegistryInfo {
            index_url: index,
            name: Some("crates-io"),
            headers: vec![],
        },
        action,
        args: vec![],
    }
}

fn publish<'a>(name: &'a str, vers: &'a str, cksum: &'a str) -> Action<'a> {
    Action::Get(Operation::Publish { name, vers, cksum })
}

fn request_credentials(
    requests: &[CredentialRequest<'_>],
    token: Option<&str>,
) -> Vec<Result<CredentialResponse, Error>> {
    let sequence = COHORT_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let cohort = env::temp_dir().join(format!(
        "bombay-release-{}-{sequence}.json",
        std::process::id()
    ));
    let ledger = serde_json::json!({CRATE_NAME: {"version": VERSION, "checksum": CHECKSUM}});
    fs::write(
        &cohort,
        serde_json::to_vec(&ledger).expect("encode fixture cohort"),
    )
    .expect("write cohort");
    let mut command = Command::new(env!("CARGO_BIN_EXE_bombay-release-credential"));
    command
        .arg("--cargo-plugin")
        .env_remove("BOMBAY_RELEASE_TOKEN");
    if let Some(token) = token {
        command.env("BOMBAY_RELEASE_TOKEN", token);
    }
    let mut child = command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("start owning SDK provider");
    let mut input = child.stdin.take().expect("provider input");
    for request in requests {
        let mut request = request.clone();
        request.args = vec![cohort.to_str().expect("fixture cohort path")];
        serde_json::to_writer(&mut input, &request).expect("owning SDK request encoding");
        input.write_all(b"\n").expect("finish SDK request");
    }
    drop(input);
    let completed = child.wait_with_output().expect("join provider");
    fs::remove_file(cohort).expect("retire fixture cohort");
    assert!(
        completed.status.success(),
        "provider failed: {:?}",
        completed.stderr
    );
    let text = String::from_utf8(completed.stdout).expect("SDK JSON text");
    let mut lines = text.lines();
    let hello: CredentialHello =
        serde_json::from_str(lines.next().expect("SDK hello")).expect("owning SDK hello decoding");
    assert_eq!(hello.v, [1]);
    lines
        .map(|line| serde_json::from_str(line).expect("owning SDK result decoding"))
        .collect()
}

#[test]
fn exact_audited_archive_receives_an_uncached_operation_bound_credential() {
    let request = credential_request(INDEX, publish(CRATE_NAME, VERSION, CHECKSUM));
    let receipts = request_credentials(&[request], Some("synthetic-test-token"));
    assert_eq!(receipts.len(), 1);
    match &receipts[0] {
        Ok(CredentialResponse::Get {
            token,
            cache: CacheControl::Never,
            operation_independent: false,
        }) => {
            assert_eq!(token.as_deref().expose(), "synthetic-test-token");
        }
        receipt => panic!("unexpected credential receipt: {receipt:?}"),
    }
}

#[test]
fn changed_archive_authority_is_refused_before_a_missing_secret_is_read() {
    for (index, name, version, checksum) in [
        (INDEX, CRATE_NAME, VERSION, "changed-archive-checksum"),
        (INDEX, "zenoh", VERSION, CHECKSUM),
        (INDEX, CRATE_NAME, "1.10.2", CHECKSUM),
        (
            "https://unrelated.example/index",
            CRATE_NAME,
            VERSION,
            CHECKSUM,
        ),
    ] {
        let request = credential_request(index, publish(name, version, checksum));
        let receipts = request_credentials(&[request], None);
        assert_eq!(receipts.len(), 1);
        let refused = receipts[0]
            .as_ref()
            .expect_err("changed authority must be refused");
        assert!(refused.to_string().contains("outside the audited release"));
    }
}

#[test]
fn reading_a_credential_does_not_authorize_a_later_changed_archive() {
    let requests = [
        credential_request(INDEX, Action::Get(Operation::Read)),
        credential_request(
            INDEX,
            publish(CRATE_NAME, VERSION, "changed-archive-checksum"),
        ),
    ];
    let receipts = request_credentials(&requests, Some("synthetic-test-token"));
    assert_eq!(receipts.len(), 2);
    assert!(matches!(
        receipts[0],
        Ok(CredentialResponse::Get {
            cache: CacheControl::Never,
            operation_independent: false,
            ..
        })
    ));
    assert!(receipts[1].is_err());
}

#[test]
fn unsupported_registry_operations_and_absent_credentials_are_explicitly_refused() {
    let requests = [
        credential_request(INDEX, Action::Logout),
        credential_request(
            INDEX,
            Action::Get(Operation::Yank {
                name: CRATE_NAME,
                vers: VERSION,
            }),
        ),
        credential_request(INDEX, publish(CRATE_NAME, VERSION, CHECKSUM)),
    ];
    let receipts = request_credentials(&requests, None);
    assert_eq!(receipts.len(), 3);
    for (receipt, diagnostic) in receipts.iter().zip([
        "outside the audited release",
        "outside the audited release",
        "credential is unavailable",
    ]) {
        let refused = receipt.as_ref().expect_err("request refused");
        assert!(refused.to_string().contains(diagnostic));
    }
}
