#![cfg(feature = "maintainer")]

use zenoh::{
    internal::traits::TimestampInstrumentationBuilderTrait, pubsub::PublicationBuilder,
    query::QuerierGetBuilder, sample::SampleBuilder, session::SessionGetBuilder,
};

fn require_timestamp_builder<T: TimestampInstrumentationBuilderTrait>(_: &T) {}

#[cfg(feature = "maintainer_macros")]
use zenoh::key_expr::{
    OwnedKeyExpr,
    format::{kedefine, keformat},
};

#[cfg(feature = "maintainer_macros")]
kedefine!(profile_keys: "profile/${node:*}/status");

#[cfg(feature = "maintainer_macros")]
pub fn key_macro_alias_paths() -> OwnedKeyExpr {
    let mut formatter = profile_keys::formatter();
    let key: OwnedKeyExpr = keformat!(formatter, node = "configured")
        .expect("existing key macro accepts its declared field");
    assert_eq!(key.as_str(), "profile/configured/status");
    key
}

pub fn timestamp_builder_implementations<Kind, Publication, ReplyDelivery>(
    sample: &SampleBuilder<Kind>,
    publication: &PublicationBuilder<Publication, Kind>,
    session_get: &SessionGetBuilder<'_, '_, ReplyDelivery>,
    querier_get: &QuerierGetBuilder<'_, '_, ReplyDelivery>,
) {
    require_timestamp_builder(sample);
    require_timestamp_builder(publication);
    require_timestamp_builder(session_get);
    require_timestamp_builder(querier_get);
}

#[cfg(feature = "maintainer_macros")]
use zenoh::{
    sample::SampleKind,
    session::{LinkEvent, TransportEvent},
};

#[cfg(feature = "maintainer_macros")]
pub fn transport_event_constructors() -> (TransportEvent, LinkEvent) {
    let transport_event = TransportEvent::empty();
    let link_event = LinkEvent::empty();
    assert_eq!(transport_event.kind(), SampleKind::Put);
    assert_eq!(link_event.kind(), SampleKind::Put);
    (transport_event, link_event)
}
