use std::num::NonZeroUsize;

use serde_json::Deserializer;
use validated_struct::{GetError, InsertionError, ValidatedMap};
use zenoh_config::{Config as ZenohConfig, GetError as ConfigGetError, ValidatedMap as ConfigMap};
use zenoh_link::{ALL_SUPPORTED_LINKS, EndPoint, LinkKind};

fn nonempty(value: &str) -> bool {
    !value.is_empty()
}

validated_struct::validator! {
    #[recursive_attrs]
    #[derive(Debug, serde::Deserialize, serde::Serialize)]
    Deployment {
        directory: String where (nonempty),
        tls: Tls {
            server: String where (nonempty),
        },
    }
}

fn main() {
    #[cfg(feature = "maintainer_macros")]
    {
        let (transport_event, link_event) =
            bombay_zenoh_configuration_consumer::transport_event_constructors();
        let _transport = transport_event.transport();
        let _link = link_event.link();
        let key = bombay_zenoh_configuration_consumer::key_macro_alias_paths();
        assert_eq!(key.as_str(), "profile/configured/status");
    }
    assert_eq!(ALL_SUPPORTED_LINKS, &[LinkKind::Tls]);
    let endpoint =
        EndPoint::new("tls", "localhost:7447", "", "").expect("owning endpoint representation");
    let supported = LinkKind::try_from(&endpoint);
    let supported = supported.expect("TLS is the selected transport");
    assert_eq!(supported, LinkKind::Tls);
    for protocol in [
        "quic",
        "serial",
        "tcp",
        "udp",
        "unixpipe",
        "unixsock-stream",
        "vsock",
        "ws",
    ] {
        let endpoint = EndPoint::new(protocol, "localhost:7447", "", "")
            .expect("well-formed unsupported endpoint");
        let selection = LinkKind::try_from(&endpoint);
        let refusal = selection.expect_err("excluded transport must not select a link");
        let expected = format!("Unicast not supported for {protocol} protocol at ");
        assert!(refusal.to_string().starts_with(&expected));
    }
    let directory = String::from("initial");
    let directory_allocation = directory.as_ptr();
    let tls = Tls::new(String::from("node.example")).expect("valid TLS configuration");
    let mut deployment = Deployment::new(directory, tls).expect("valid deployment");
    assert_eq!(deployment.directory().as_ptr(), directory_allocation);
    assert_eq!(deployment.tls().server(), "node.example");

    let previous = deployment
        .set_directory(String::from("replacement"))
        .expect("valid replacement");
    assert_eq!(previous.as_ptr(), directory_allocation);
    assert_eq!(previous, "initial");
    let invalid = String::with_capacity(16);
    let invalid_allocation = invalid.as_ptr();
    let rejected = deployment
        .set_directory(invalid)
        .expect_err("empty directory rejected");
    assert_eq!(rejected.as_ptr(), invalid_allocation);
    assert!(rejected.is_empty());
    assert_eq!(deployment.directory(), "replacement");

    let invalid = String::with_capacity(16);
    let invalid_allocation = invalid.as_ptr();
    let tls = Tls::new(String::from("retained.example")).expect("valid retained TLS configuration");
    let rejected =
        Deployment::new(invalid, tls).expect_err("invalid constructor returns deployment");
    assert_eq!(rejected.directory().as_ptr(), invalid_allocation);
    assert_eq!(rejected.tls().server(), "retained.example");

    let mut valid_json =
        Deserializer::from_str(r#"{"directory":"loaded","tls":{"server":"loaded.example"}}"#);
    let mut loaded =
        Deployment::from_deserializer(&mut valid_json).expect("checked nested configuration");
    assert_eq!(loaded.directory(), "loaded");
    assert_eq!(loaded.tls().server(), "loaded.example");
    let mut invalid_json = Deserializer::from_str(r#"{"directory":"loaded","tls":{"server":""}}"#);
    let rejected = Deployment::from_deserializer(&mut invalid_json)
        .expect_err("invalid nested field rejected");
    let rejected = rejected.expect("invalid configuration retains deserialized value");
    assert_eq!(rejected.directory(), "loaded");
    assert!(rejected.tls().server().is_empty());

    let insertion: Result<(), InsertionError> =
        loaded.insert_json5("tls/server", "'updated.example'");
    insertion.expect("existing validated map accepts nested insertion");
    let lookup = <Deployment as ValidatedMap>::get(&loaded, "tls/server")
        .expect("existing validated map returns the nested field");
    let server = lookup
        .downcast_ref::<String>()
        .expect("existing String field");
    assert_eq!(server.as_ptr(), loaded.tls().server().as_ptr());
    assert_eq!(server, "updated.example");
    let json: Result<String, GetError> =
        <Deployment as ValidatedMap>::get_json(&loaded, "tls/server");
    let json = json.expect("nested JSON projection");
    assert_eq!(json, "\"updated.example\"");
    let missing: Result<_, GetError> = <Deployment as ValidatedMap>::get(&loaded, "absent");
    assert!(matches!(missing, Err(GetError::NoMatchingKey)));
    let keys = <Deployment as ValidatedMap>::keys(&loaded);
    assert_eq!(keys, ["directory", "tls", "tls/server"]);

    let mut config = ZenohConfig::default();
    assert_eq!(config.queries_max_retained(), &None);
    let retained = NonZeroUsize::new(1).expect("explicit retained-query capacity");
    let previous: Result<Option<NonZeroUsize>, Option<NonZeroUsize>> =
        config.set_queries_max_retained(Some(retained));
    let previous = previous.expect("owning validated setter accepts capacity");
    assert_eq!(previous, None);
    assert_eq!(config.queries_max_retained(), &Some(retained));
    let zero = config.insert_json5("queries_max_retained", "0");
    let _zero_refusal = zero.expect_err("zero cannot erase the retained-query limit");
    assert_eq!(config.queries_max_retained(), &Some(retained));
    let insertion: Result<(), InsertionError> =
        config.insert_json5("metadata", "{ configured: true }");
    insertion.expect("Zenoh configuration uses the corrected runtime error");
    let metadata: Result<String, ConfigGetError> =
        <ZenohConfig as ConfigMap>::get_json(&config, "metadata");
    let metadata: Result<String, GetError> = metadata;
    let metadata = metadata.expect("Zenoh reexport retains the corrected error identity");
    assert_eq!(metadata, r#"{"configured":true}"#);
    let missing: Result<_, GetError> = <ZenohConfig as ValidatedMap>::get(&config, "absent");
    assert!(matches!(missing, Err(ConfigGetError::NoMatchingKey)));
    let mut config: zenoh::Config = config.into();
    let metadata = config.get_json("metadata");
    let metadata = metadata.expect("top-level configuration retains owner identity");
    assert_eq!(metadata, r#"{"configured":true}"#);
    let configured = config.insert_json5("transport/link/tls/use_public_pki", "false");
    configured.expect("top-level configuration supports the approved trust selection");
    let selection = config
        .get_json("transport/link/tls/use_public_pki")
        .expect("top-level configuration preserves the explicit trust selection");
    assert_eq!(selection, "false");
    println!("configuration consumer assertions passed");
}
