//
// Copyright (c) 2026 ZettaScale Technology
//
// This program and the accompanying materials are made available under the
// terms of the Eclipse Public License 2.0 which is available at
// http://www.eclipse.org/legal/epl-2.0, or the Apache License, Version 2.0
// which is available at https://www.apache.org/licenses/LICENSE-2.0.
//
// SPDX-License-Identifier: EPL-2.0 OR Apache-2.0
//
// Contributors:
//   ZettaScale Zenoh Team, <zenoh@zettascale.tech>
//

//! Tests involving [`zenoh_protocol::network::interest`].

use std::sync::{Arc, Weak};

use zenoh_protocol::{
    core::{Bound, Region, WhatAmI},
    network::{
        declare::{queryable::ext::QueryableInfoType, DeclareToken, TokenId},
        interest::{InterestMode, InterestOptions},
    },
};
use zenoh_transport::TransportPeerEventHandler;

use super::{try_init_tracing_subscriber, Connection, FaceDef, Harness, HarnessBuilder};
use crate::net::{
    primitives::Primitives,
    routing::{dispatcher::resource::Resource, gateway::Gateway},
};

const INTEREST_NAMESPACE: &str = "interest-retirement";

fn interest_namespace_inventory(gateway: &Gateway) -> (usize, usize) {
    let tables = gateway.tables.tables.read().unwrap();
    let mut resources = vec![tables.data._get_root().as_ref()];
    let mut nodes = 0;
    let mut expression_bytes = 0;
    // Borrowing the native tree does not perturb strong-count reclamation.
    while let Some(resource) = resources.pop() {
        if resource
            .expr()
            .strip_prefix(INTEREST_NAMESPACE)
            .is_some_and(|suffix| suffix.is_empty() || suffix.starts_with('/'))
        {
            nodes += 1;
            expression_bytes += resource.expr().len();
        }
        resources.extend(resource.children.iter().map(|child| child.as_ref()));
    }
    (nodes, expression_bytes)
}

fn interest_resource(gateway: &Gateway, expression: &str) -> Weak<Resource> {
    let tables = gateway.tables.tables.read().unwrap();
    let resource = Resource::get_resource_ref(tables.data._get_root(), expression)
        .expect("the actual interest established its resource");
    Arc::downgrade(resource)
}

#[test]
fn interest_final_releases_resources_across_identifier_reuse_and_replay() {
    let gateway = HarnessBuilder::new()
        .mode(WhatAmI::Client)
        .subregions([Region::Local])
        .start_runtime(false)
        .build();
    let face = gateway.new_session();
    let baseline = interest_namespace_inventory(&gateway.gateway);
    assert_eq!(baseline, (0, 0));
    let mut originals = Vec::new();
    for expression in ["interest-retirement/first", "interest-retirement/second"] {
        face.interest(
            42,
            InterestMode::Future,
            InterestOptions::KEYEXPRS,
            expression,
        );
        let original = interest_resource(&gateway.gateway, expression);
        assert_eq!(face.face.state.remote_key_interests.len(), 1);
        assert!(original.strong_count() > 0);
        originals.push(original);
        face.interest_wildcard(42, InterestMode::Final, InterestOptions::KEYEXPRS);
        assert_eq!(face.face.state.remote_key_interests.len(), 0);
        // Replay the same finalization before reusing its now-retired identifier.
        face.interest_wildcard(42, InterestMode::Final, InterestOptions::KEYEXPRS);
    }
    let finalized = interest_namespace_inventory(&gateway.gateway);
    let routed_messages = face.recorder().with_messages(|messages| messages.len());
    face.face.send_close();
    drop(face);
    assert_eq!(routed_messages, 0, "this gateway has no north-bound peer");
    for original in originals {
        let retained = original.upgrade();
        assert!(
            retained.is_none(),
            "finalized interest retains its original resource"
        );
    }
    assert_eq!(
        finalized, baseline,
        "finalized interest retains namespace nodes or name bytes"
    );
}

#[test_case::test_matrix(
    [WhatAmI::Client, WhatAmI::Peer],
    [(InterestOptions::KEYEXPRS, 1), (InterestOptions::SUBSCRIBERS, 0)]
)]
fn interest_final_preserves_shared_resources_until_the_last_owner_finishes(
    remote_mode: WhatAmI,
    (options, key_interest_count): (InterestOptions, usize),
) {
    let region = Region::default_south(remote_mode);
    let gateway = HarnessBuilder::new()
        .mode(WhatAmI::Client)
        .subregions([region])
        .build();
    let face_definition = FaceDef::default().mode(remote_mode).region(region);
    let first = gateway.new_face(face_definition);
    let second = gateway.new_face(face_definition);
    let expression = "interest-retirement/shared";
    first.interest(42, InterestMode::Future, options, expression);
    second.interest(42, InterestMode::Future, options, expression);
    let original = interest_resource(&gateway.gateway, expression);
    let active = interest_namespace_inventory(&gateway.gateway);
    assert_eq!(active, (2, INTEREST_NAMESPACE.len() + expression.len()));
    first.interest_wildcard(42, InterestMode::Final, options);
    let shared = interest_namespace_inventory(&gateway.gateway);
    assert_eq!(
        shared, active,
        "the other face still owns this exact resource"
    );
    assert!(original.strong_count() > 0);
    assert_eq!(first.face.state.remote_key_interests.len(), 0);
    assert_eq!(
        second.face.state.remote_key_interests.len(),
        key_interest_count
    );
    second.interest_wildcard(42, InterestMode::Final, options);
    let finalized = interest_namespace_inventory(&gateway.gateway);
    first.face.send_close();
    second.face.send_close();
    drop(first);
    drop(second);
    let retained = original.upgrade();
    assert!(
        retained.is_none(),
        "last finalized owner retains its original resource"
    );
    assert_eq!(
        finalized,
        (0, 0),
        "last finalized owner retains namespace nodes or name bytes"
    );
}

#[test_case::test_matrix(
    [WhatAmI::Client, WhatAmI::Peer],
    [(InterestOptions::KEYEXPRS, 1), (InterestOptions::SUBSCRIBERS, 0)]
)]
fn interest_disconnect_releases_resources_after_the_native_face_is_removed(
    remote_mode: WhatAmI,
    (options, key_interest_count): (InterestOptions, usize),
) {
    let region = Region::default_south(remote_mode);
    let gateway = HarnessBuilder::new()
        .mode(WhatAmI::Client)
        .subregions([region])
        .build();
    let face = gateway.new_face(FaceDef::default().mode(remote_mode).region(region));
    let face_id = face.face.state.id;
    let expression = "interest-retirement/disconnected";
    face.interest(42, InterestMode::Future, options, expression);
    let original = interest_resource(&gateway.gateway, expression);
    let active = interest_namespace_inventory(&gateway.gateway);
    assert_eq!(active, (2, INTEREST_NAMESPACE.len() + expression.len()));
    assert_eq!(
        face.face.state.remote_key_interests.len(),
        key_interest_count
    );
    face.demux
        .as_ref()
        .expect("the real native transport callback")
        .closed();
    let tables = gateway.gateway.tables.tables.read().unwrap();
    assert!(!tables.data.faces.contains_key(&face_id));
    drop(tables);
    drop(face);
    let disconnected = interest_namespace_inventory(&gateway.gateway);
    let retained = original.upgrade();
    assert!(
        retained.is_none(),
        "disconnected interest retains its original resource"
    );
    assert_eq!(
        disconnected,
        (0, 0),
        "disconnected interest retains namespace nodes or name bytes"
    );
}

#[test_case::test_matrix([WhatAmI::Client, WhatAmI::Peer])]
fn wildcard_interest_final_releases_native_registrations_without_allocating_resources(
    remote_mode: WhatAmI,
) {
    let region = Region::default_south(remote_mode);
    let gateway = HarnessBuilder::new()
        .mode(WhatAmI::Client)
        .subregions([region])
        .build();
    let face = gateway.new_face(FaceDef::default().mode(remote_mode).region(region));
    face.interest_wildcard(42, InterestMode::Future, InterestOptions::KEYEXPRS);
    assert!(matches!(
        face.face.state.remote_key_interests.get(&42),
        Some(None)
    ));
    let tables = gateway.gateway.tables.tables.read().unwrap();
    let interests = tables.hats[region].remote_interests(&tables.data);
    assert_eq!(interests.len(), 1);
    assert!(interests.iter().all(|interest| interest.res.is_none()));
    drop(interests);
    drop(tables);
    face.interest_wildcard(42, InterestMode::Final, InterestOptions::KEYEXPRS);
    face.interest_wildcard(42, InterestMode::Final, InterestOptions::KEYEXPRS);
    assert_eq!(face.face.state.remote_key_interests.len(), 0);
    let tables = gateway.gateway.tables.tables.read().unwrap();
    let interests = tables.hats[region].remote_interests(&tables.data);
    assert_eq!(interests.len(), 0);
    drop(interests);
    drop(tables);
    let finalized = interest_namespace_inventory(&gateway.gateway);
    face.demux
        .as_ref()
        .expect("the native remote face")
        .closed();
    drop(face);
    assert_eq!(finalized, (0, 0));
}

/// Test that current tokens are re-propagated even if they've already been propagated in future
/// mode.
///
/// ```d2
/// shape: sequence_diagram
///
/// C1 -> R: Interest id=1 mode=F
///
/// C2 -> R.1: DeclareToken iid=None
/// R.1 -> C1: DeclareToken iid=None
///
/// C1 -> R.2: Interest id=2 mode=C
/// R.2 -> C1: DeclareToken iid=2
/// R.2 -> C1: DeclareFinal iid=2
/// ```
#[test]
fn test_current_token_repropagation() {
    try_init_tracing_subscriber();

    let r = Harness::new_router();
    let c1 = Harness::new_client();
    let c2 = Harness::new_client();

    let s1 = c1.new_session();
    let s2 = c1.new_session();

    let r_face = FaceDef::default()
        .region(Region::North)
        .remote_bound(Bound::South)
        .mode(WhatAmI::Router);

    let c_face = FaceDef::default()
        .mode(WhatAmI::Client)
        .region(Region::default_south(WhatAmI::Client));

    let mut c1_r = Connection {
        a: &c1,
        a2b: r_face,
        b: &r,
        b2a: c_face,
    }
    .establish();
    c1_r.bi_fwd();

    let mut c2_r = Connection {
        a: &c2,
        a2b: r_face,
        b: &r,
        b2a: c_face,
    }
    .establish();
    c2_r.bi_fwd();

    s1.interest(1, InterestMode::Future, InterestOptions::TOKENS, "test");
    c1_r.bi_fwd();

    s2.declare_token(None, 1, "test");
    c2_r.bi_fwd();
    c1_r.bi_fwd();

    assert_eq!(
        s1.recorder().tokens().as_slice(),
        &[DeclareToken {
            id: 1,
            wire_expr: "test".into(),
        }]
    );

    s1.interest(1, InterestMode::Current, InterestOptions::TOKENS, "test");
    c1_r.bi_fwd();

    assert_eq!(
        s1.recorder().tokens().as_slice(),
        &[
            DeclareToken {
                id: 1,
                wire_expr: "test".into(),
            },
            DeclareToken {
                id: TokenId::default(),
                wire_expr: "test".into(),
            }
        ]
    );
}

/// Test peer-to-peer interest routing in the presence of unfinalized initial interests.
///
/// This checks for a regression discovered in RMW Zenoh which uses peer mode and sends a
/// [liveliness GET] right after opening a session.
///
/// This issue occurred because we did not check that the source of a current token's interest is
/// south-bound before propagating it to peers with unfinalized initial interests.
///
/// [liveliness GET]:
///     https://github.com/ros2/rmw_zenoh/blob/944a8715f5af6f58e74e318d31510409f69a5e6e/rmw_zenoh_cpp/src/detail/rmw_context_impl_s.cpp#L250-L254
#[test]
fn test_p2p_interest_routing_with_unfinalized_initial_interests() {
    try_init_tracing_subscriber();

    let p = HarnessBuilder::new()
        .mode(WhatAmI::Peer)
        .subregions([])
        .build();

    let p0 = p.new_face(FaceDef::default().mode(WhatAmI::Peer));
    let p1 = p.new_face(FaceDef::default().mode(WhatAmI::Peer));

    assert_eq!(p0.recorder().declare_finals().len(), 1);
    assert_eq!(p1.recorder().declare_finals().len(), 1);

    p0.interest_wildcard(42, InterestMode::Current, InterestOptions::TOKENS);

    assert_eq!(p0.recorder().tokens().len(), 0);

    assert_eq!(p0.recorder().declare_finals().len(), 2);
    assert_eq!(p1.recorder().declare_finals().len(), 1);

    assert!(p0.recorder().interests().is_empty());
    assert!(p1.recorder().interests().is_empty());
}

/// Same as [`test_p2p_interest_routing_with_unfinalized_initial_interests`] but finalizes initial
/// interests before sending the current tokens interest.
#[test]
fn test_p2p_interest_routing_with_finalized_initial_interests() {
    let p = HarnessBuilder::new()
        .mode(WhatAmI::Peer)
        .subregions([])
        .build();

    let p0 = p.new_face(FaceDef::default().mode(WhatAmI::Peer));
    let p1 = p.new_face(FaceDef::default().mode(WhatAmI::Peer));

    p0.declare_final(0);
    p1.declare_final(0);

    assert_eq!(p0.recorder().declare_finals().len(), 1);
    assert_eq!(p1.recorder().declare_finals().len(), 1);

    p0.interest_wildcard(42, InterestMode::Current, InterestOptions::TOKENS);

    assert_eq!(p0.recorder().tokens().len(), 0);

    assert_eq!(p0.recorder().declare_finals().len(), 2);
    assert_eq!(p1.recorder().declare_finals().len(), 1);

    assert!(p0.recorder().interests().is_empty());
    assert!(p1.recorder().interests().is_empty());
}

/// Concurrent current-future interest in a two-region hierarchy.
#[test_case::test_matrix(
    [WhatAmI::Client, WhatAmI::Peer],
    [WhatAmI::Client, WhatAmI::Peer]
)]
fn test_concurrent_current_future_interests(north: WhatAmI, south: WhatAmI) {
    try_init_tracing_subscriber();

    let r = Region::default_south(south);
    let g = HarnessBuilder::new().mode(north).subregions([r]).build();
    let n = g.new_face(FaceDef::default().remote_bound(Bound::South));
    let s0 = g.new_face(FaceDef::default().mode(south).region(r));
    let s1 = g.new_face(FaceDef::default().mode(south).region(r));

    assert_eq!(n.recorder().interests().len(), 0);

    s0.interest_wildcard(
        42,
        InterestMode::CurrentFuture,
        InterestOptions::KEYEXPRS + InterestOptions::SUBSCRIBERS,
    );

    s1.interest_wildcard(
        42,
        InterestMode::CurrentFuture,
        InterestOptions::KEYEXPRS + InterestOptions::SUBSCRIBERS,
    );

    assert_eq!(n.recorder().interests().len(), 2);

    n.declare_subscriber(Some(42), 1999, "k");

    assert_eq!(s0.recorder().subscribers().len(), 1);
    assert_eq!(s1.recorder().subscribers().len(), 1);
}

/// Re-propagated current-future interest in a two-region hierarchy.
#[test_case::test_matrix(
    [WhatAmI::Client, WhatAmI::Peer],
    [WhatAmI::Client, WhatAmI::Peer]
)]
/// Test that current-future interests are propagated upstresam on open and that downstream
/// declarations with interest id are accepted by the middle gateway even though there is no
/// breadcrumb.
fn test_current_future_interest_propagation_on_open(north: WhatAmI, south: WhatAmI) {
    try_init_tracing_subscriber();

    let r = Region::default_south(south);
    let g = HarnessBuilder::new().mode(north).subregions([r]).build();
    let s = g.new_face(FaceDef::default().mode(south).region(r));

    s.interest_wildcard(42, InterestMode::CurrentFuture, InterestOptions::QUERYABLES);

    let n = g.new_face(FaceDef::default().remote_bound(Bound::South));

    assert_eq!(n.recorder().interests().len(), 1);
    assert_eq!(s.recorder().queryables().len(), 0);

    n.declare_queryable(Some(42), 1999, "k", QueryableInfoType::DEFAULT);

    assert_eq!(s.recorder().queryables().len(), 1);
}

/// Test that gateways send back declare final if there is no upstream.
#[test_case::test_matrix([WhatAmI::Client, WhatAmI::Peer, WhatAmI::Router])]
fn test_current_interest_finalization(mode: WhatAmI) {
    let g = HarnessBuilder::new()
        .mode(mode)
        .subregions([Region::Local])
        .build();

    let s = g.new_session();

    s.interest_wildcard(2, InterestMode::CurrentFuture, InterestOptions::ALL);

    assert_eq!(s.recorder().declare_finals().len(), 1)
}
