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

//! Tests involving [`zenoh_protocol::network::push`] & [`zenoh_protocol::network::request`], i.e.
//! the data/forwarding plane.

use futures::FutureExt;
use std::{sync::Arc, time::Duration};
use zenoh_sync::get_mut_unchecked;

use zenoh_protocol::{
    core::{Bound, Region, Reliability, WhatAmI, WireExpr},
    network::{
        declare::queryable::ext::QueryableInfoType,
        ext as network_ext,
        interest::{InterestMode, InterestOptions},
        request::ext::QueryTarget,
        Mapping, Push, Request, Response, ResponseFinal,
    },
    zenoh::{ConsolidationMode, PushBody, Put, Query, Reply, RequestBody, ResponseBody},
};

use super::{try_init_tracing_subscriber, FaceDef, HarnessBuilder};
use crate::api::builders::close::CloseBuilder;
use crate::net::{
    primitives::Primitives,
    tests::regions::{Connection, EstablishedConnection},
};

/// Tests data routing between peer subregions.
///
/// Here we validate that the dispatcher properly stores different routes per (sub-)region.
///
/// ```d2
/// shape: sequence_diagram
///
/// P0 -> G: Open
/// P1 -> G: Open
///
/// # P0 and P1 belong in two distinct sub-regions of G (S0 and S1 resp.)
///
/// P0 -> G: DeclareSubscriber expr=K
/// P1 -> G: DeclareSubscriber expr=K
///
/// P0 -> G.0: Push expr=K
/// G.0 -> P1: Push expr=K
/// # G caches an empty routes object for data originating in S0
///
/// P1 -> G.1: Push expr=K
/// G.1 -> P0: Push expr=K
/// # G caches an empty routes object for data originating in S1
/// ```
#[test]
fn test_p2p_inter_subregion_data_routing() {
    try_init_tracing_subscriber();

    const S1: Region = Region::South {
        id: 0,
        mode: WhatAmI::Peer,
    };

    const S2: Region = Region::South {
        id: 1,
        mode: WhatAmI::Peer,
    };

    let g = HarnessBuilder::new()
        .mode(WhatAmI::default())
        .subregions([S1, S2])
        .build();

    let p0 = g.new_face(FaceDef::default().mode(WhatAmI::Peer).region(S1));
    let p1 = g.new_face(FaceDef::default().mode(WhatAmI::Peer).region(S2));

    let ke = "k";

    p0.declare_subscriber(None, 1, ke);
    p1.declare_subscriber(None, 1, ke);

    p0.put(ke, vec![42]);
    p1.put(ke, vec![43]);

    assert_eq!(p0.recorder().pushes().len(), 1);
    assert_eq!(p0.recorder().pushes().len(), 1);
}

/// Same as [`test_p2p_inter_subregion_data_routing`] but for queries.
#[test]
fn test_p2p_inter_subregion_query_routing() {
    try_init_tracing_subscriber();

    const S1: Region = Region::South {
        id: 0,
        mode: WhatAmI::Peer,
    };

    const S2: Region = Region::South {
        id: 1,
        mode: WhatAmI::Peer,
    };

    let g = HarnessBuilder::new()
        .mode(WhatAmI::default())
        .subregions([S1, S2])
        .build();

    let p0 = g.new_face(FaceDef::default().mode(WhatAmI::Peer).region(S1));
    let p1 = g.new_face(FaceDef::default().mode(WhatAmI::Peer).region(S2));

    let ke = "k";

    p0.declare_queryable(None, 1, ke, QueryableInfoType::DEFAULT);
    p1.declare_queryable(None, 1, ke, QueryableInfoType::DEFAULT);

    p0.query(1, ke);
    p1.query(1, ke);

    assert_eq!(p0.recorder().requests().len(), 1);
    assert_eq!(p0.recorder().requests().len(), 1);
}

/// Tests data routing between router subregions.
#[test]
fn test_r2r_inter_subregion_data_routing() {
    try_init_tracing_subscriber();

    const S1: Region = Region::South {
        id: 0,
        mode: WhatAmI::Router,
    };

    const S2: Region = Region::South {
        id: 1,
        mode: WhatAmI::Router,
    };

    let g = HarnessBuilder::new()
        .mode(WhatAmI::default())
        .subregions([S1, S2])
        .build();
    let r0 = HarnessBuilder::new()
        .mode(WhatAmI::Router)
        .subregions([Region::Local])
        .build();
    let r1 = HarnessBuilder::new()
        .mode(WhatAmI::Router)
        .subregions([Region::Local])
        .build();

    let r0s = r0.new_session();
    let r1s = r1.new_session();

    let mut r0_g = Connection {
        a: &r0,
        a2b: FaceDef::default()
            .mode(WhatAmI::Router)
            .remote_bound(Bound::South),
        b: &g,
        b2a: FaceDef::default().mode(WhatAmI::Router).region(S1),
    }
    .establish();

    let mut r1_g = Connection {
        a: &r1,
        a2b: FaceDef::default()
            .mode(WhatAmI::Router)
            .remote_bound(Bound::South),
        b: &g,
        b2a: FaceDef::default().mode(WhatAmI::Router).region(S2),
    }
    .establish();

    let mut bi_fwd_all = || EstablishedConnection::bi_fwd_many_unbounded([&mut r0_g, &mut r1_g]);

    bi_fwd_all();

    let ke = "k";

    r0s.declare_subscriber(None, 1, ke);
    r1s.declare_subscriber(None, 1, ke);
    bi_fwd_all();

    r0s.put(ke, vec![42]);
    bi_fwd_all();

    r1s.put(ke, vec![43]);
    bi_fwd_all();

    assert_eq!(r1s.recorder().pushes().len(), 1);
    assert_eq!(r0s.recorder().pushes().len(), 1);
}

/// Tests query routing between router subregions.
#[test]
fn test_r2r_inter_subregion_query_routing() {
    try_init_tracing_subscriber();

    const S1: Region = Region::South {
        id: 0,
        mode: WhatAmI::Router,
    };

    const S2: Region = Region::South {
        id: 1,
        mode: WhatAmI::Router,
    };

    let g = HarnessBuilder::new()
        .mode(WhatAmI::default())
        .subregions([S1, S2])
        .build();
    let r0 = HarnessBuilder::new()
        .mode(WhatAmI::Router)
        .subregions([Region::Local])
        .build();
    let r1 = HarnessBuilder::new()
        .mode(WhatAmI::Router)
        .subregions([Region::Local])
        .build();

    let r0s = r0.new_session();
    let r1s = r1.new_session();

    let mut r0_g = Connection {
        a: &r0,
        a2b: FaceDef::default()
            .mode(WhatAmI::Router)
            .remote_bound(Bound::South),
        b: &g,
        b2a: FaceDef::default().mode(WhatAmI::Router).region(S1),
    }
    .establish();

    let mut r1_g = Connection {
        a: &r1,
        a2b: FaceDef::default()
            .mode(WhatAmI::Router)
            .remote_bound(Bound::South),
        b: &g,
        b2a: FaceDef::default().mode(WhatAmI::Router).region(S2),
    }
    .establish();

    let mut bi_fwd_all = || EstablishedConnection::bi_fwd_many_unbounded([&mut r0_g, &mut r1_g]);

    bi_fwd_all();

    let ke = "k";

    r0s.declare_queryable(None, 1, ke, QueryableInfoType::DEFAULT);
    r1s.declare_queryable(None, 1, ke, QueryableInfoType::DEFAULT);
    bi_fwd_all();

    r0s.query(1, ke);
    bi_fwd_all();

    r1s.query(1, ke);
    bi_fwd_all();

    assert_eq!(r1s.recorder().requests().len(), 1);
    assert_eq!(r0s.recorder().requests().len(), 1);
}

/// Tests data routing between client subregions.
#[test]
fn test_c2c_inter_subregion_data_routing() {
    try_init_tracing_subscriber();

    const S1: Region = Region::South {
        id: 0,
        mode: WhatAmI::Client,
    };

    const S2: Region = Region::South {
        id: 1,
        mode: WhatAmI::Client,
    };

    let g = HarnessBuilder::new()
        .mode(WhatAmI::default())
        .subregions([S1, S2])
        .build();

    let c0 = g.new_face(FaceDef::default().mode(WhatAmI::Client).region(S1));
    let c1 = g.new_face(FaceDef::default().mode(WhatAmI::Client).region(S2));

    let ke = "k";

    c0.declare_subscriber(None, 1, ke);
    c1.declare_subscriber(None, 1, ke);

    c0.put(ke, vec![42]);
    c1.put(ke, vec![43]);

    assert_eq!(c0.recorder().pushes().len(), 1);
    assert_eq!(c1.recorder().pushes().len(), 1);
}

/// Tests query routing between client subregions.
#[test]
fn test_c2c_inter_subregion_query_routing() {
    try_init_tracing_subscriber();

    const S1: Region = Region::South {
        id: 0,
        mode: WhatAmI::Client,
    };

    const S2: Region = Region::South {
        id: 1,
        mode: WhatAmI::Client,
    };

    let g = HarnessBuilder::new()
        .mode(WhatAmI::default())
        .subregions([S1, S2])
        .build();

    let c0 = g.new_face(FaceDef::default().mode(WhatAmI::Client).region(S1));
    let c1 = g.new_face(FaceDef::default().mode(WhatAmI::Client).region(S2));

    let ke = "k";

    c0.declare_queryable(None, 1, ke, QueryableInfoType::DEFAULT);
    c1.declare_queryable(None, 1, ke, QueryableInfoType::DEFAULT);

    c0.query(1, ke);
    c1.query(1, ke);

    assert_eq!(c0.recorder().requests().len(), 1);
    assert_eq!(c1.recorder().requests().len(), 1);
}

#[test]
fn test_multiple_gateways_data_routing_r2p_downstream() {
    try_init_tracing_subscriber();

    let g0 = HarnessBuilder::new()
        .mode(WhatAmI::Router)
        .subregions([Region::default_south(WhatAmI::Peer)])
        .build();
    let g1 = HarnessBuilder::new()
        .mode(WhatAmI::Router)
        .subregions([Region::default_south(WhatAmI::Peer)])
        .build();
    let r = HarnessBuilder::new()
        .mode(WhatAmI::Router)
        .subregions([Region::Local])
        .build();
    let p = HarnessBuilder::new()
        .mode(WhatAmI::Peer)
        .subregions([Region::Local])
        .build();

    let ps = p.new_session();
    let rs = r.new_session();

    let mut r_g0 = Connection {
        a: &r,
        a2b: FaceDef::default().mode(WhatAmI::Router),
        b: &g0,
        b2a: FaceDef::default().mode(WhatAmI::Router),
    }
    .establish();

    let mut r_g1 = Connection {
        a: &r,
        a2b: FaceDef::default().mode(WhatAmI::Router),
        b: &g1,
        b2a: FaceDef::default().mode(WhatAmI::Router),
    }
    .establish();

    let mut p_g0 = Connection {
        a: &p,
        a2b: FaceDef::default()
            .mode(WhatAmI::Router)
            .remote_bound(Bound::South),
        b: &g0,
        b2a: FaceDef::default()
            .mode(WhatAmI::Peer)
            .region(Region::default_south(WhatAmI::Peer)),
    }
    .establish();

    let mut p_g1 = Connection {
        a: &p,
        a2b: FaceDef::default()
            .mode(WhatAmI::Router)
            .remote_bound(Bound::South),
        b: &g1,
        b2a: FaceDef::default()
            .mode(WhatAmI::Peer)
            .region(Region::default_south(WhatAmI::Peer)),
    }
    .establish();

    let mut bi_fwd_all = || {
        EstablishedConnection::bi_fwd_many_unbounded([&mut r_g0, &mut r_g1, &mut p_g0, &mut p_g1])
    };

    bi_fwd_all();

    let ke = "k";

    ps.declare_subscriber(None, 1, ke);
    bi_fwd_all();

    rs.put(ke, vec![42]);
    bi_fwd_all();

    assert_eq!(ps.recorder().pushes().len(), 1);

    assert!(r_g0.is_bi_complete());
    assert!(r_g1.is_bi_complete());
    assert!(p_g0.is_bi_complete());
    assert!(p_g1.is_bi_complete());
}

#[test]
fn test_multiple_gateways_query_routing_r2p_downstream() {
    try_init_tracing_subscriber();

    let g0 = HarnessBuilder::new()
        .mode(WhatAmI::Router)
        .subregions([Region::default_south(WhatAmI::Peer)])
        .build();
    let g1 = HarnessBuilder::new()
        .mode(WhatAmI::Router)
        .subregions([Region::default_south(WhatAmI::Peer)])
        .build();
    let r = HarnessBuilder::new()
        .mode(WhatAmI::Router)
        .subregions([Region::Local])
        .build();
    let p = HarnessBuilder::new()
        .mode(WhatAmI::Peer)
        .subregions([Region::Local])
        .build();

    let ps = p.new_session();
    let rs = r.new_session();

    let mut r_g0 = Connection {
        a: &r,
        a2b: FaceDef::default().mode(WhatAmI::Router),
        b: &g0,
        b2a: FaceDef::default().mode(WhatAmI::Router),
    }
    .establish();

    let mut r_g1 = Connection {
        a: &r,
        a2b: FaceDef::default().mode(WhatAmI::Router),
        b: &g1,
        b2a: FaceDef::default().mode(WhatAmI::Router),
    }
    .establish();

    let mut p_g0 = Connection {
        a: &p,
        a2b: FaceDef::default()
            .mode(WhatAmI::Router)
            .remote_bound(Bound::South),
        b: &g0,
        b2a: FaceDef::default()
            .mode(WhatAmI::Peer)
            .region(Region::default_south(WhatAmI::Peer)),
    }
    .establish();

    let mut p_g1 = Connection {
        a: &p,
        a2b: FaceDef::default()
            .mode(WhatAmI::Router)
            .remote_bound(Bound::South),
        b: &g1,
        b2a: FaceDef::default()
            .mode(WhatAmI::Peer)
            .region(Region::default_south(WhatAmI::Peer)),
    }
    .establish();

    let mut bi_fwd_all = || {
        EstablishedConnection::bi_fwd_many_unbounded([&mut r_g0, &mut r_g1, &mut p_g0, &mut p_g1])
    };

    bi_fwd_all();

    let ke = "k";

    ps.declare_queryable(None, 1, ke, QueryableInfoType::DEFAULT);
    bi_fwd_all();

    rs.query(1, ke);
    bi_fwd_all();

    assert_eq!(ps.recorder().requests().len(), 1);

    assert!(r_g0.is_bi_complete());
    assert!(r_g1.is_bi_complete());
    assert!(p_g0.is_bi_complete());
    assert!(p_g1.is_bi_complete());
}

#[test]
fn test_multiple_gateways_data_routing_r2r_downstream() {
    try_init_tracing_subscriber();

    let g0 = HarnessBuilder::new()
        .zid("a0".parse().unwrap())
        .mode(WhatAmI::Router)
        .subregions([Region::default_south(WhatAmI::Router)])
        .build();
    let g1 = HarnessBuilder::new()
        .zid("a1".parse().unwrap())
        .mode(WhatAmI::Router)
        .subregions([Region::default_south(WhatAmI::Router)])
        .build();
    let n = HarnessBuilder::new()
        .zid("a".parse().unwrap())
        .mode(WhatAmI::Router)
        .subregions([Region::Local])
        .build();
    let s = HarnessBuilder::new()
        .zid("b".parse().unwrap())
        .mode(WhatAmI::Router)
        .subregions([Region::Local])
        .build();

    let ss = s.new_session();
    let ns = n.new_session();

    let mut n_g0 = Connection {
        a: &n,
        a2b: FaceDef::default().mode(WhatAmI::Router),
        b: &g0,
        b2a: FaceDef::default().mode(WhatAmI::Router),
    }
    .establish();

    let mut n_g1 = Connection {
        a: &n,
        a2b: FaceDef::default().mode(WhatAmI::Router),
        b: &g1,
        b2a: FaceDef::default().mode(WhatAmI::Router),
    }
    .establish();

    let mut s_g0 = Connection {
        a: &s,
        a2b: FaceDef::default()
            .mode(WhatAmI::Router)
            .remote_bound(Bound::South),
        b: &g0,
        b2a: FaceDef::default()
            .mode(WhatAmI::Router)
            .region(Region::default_south(WhatAmI::Router)),
    }
    .establish();

    let mut s_g1 = Connection {
        a: &s,
        a2b: FaceDef::default()
            .mode(WhatAmI::Router)
            .remote_bound(Bound::South),
        b: &g1,
        b2a: FaceDef::default()
            .mode(WhatAmI::Router)
            .region(Region::default_south(WhatAmI::Router)),
    }
    .establish();

    let mut bi_fwd_all = || {
        EstablishedConnection::bi_fwd_many_unbounded([&mut n_g0, &mut n_g1, &mut s_g0, &mut s_g1])
    };

    bi_fwd_all();

    let ke = "k";

    ss.declare_subscriber(None, 1, ke);
    bi_fwd_all();

    ns.put(ke, vec![0x42]);
    bi_fwd_all();

    assert_eq!(ss.recorder().pushes().len(), 1);

    assert!(n_g0.is_bi_complete());
    assert!(n_g1.is_bi_complete());
    assert!(s_g0.is_bi_complete());
    assert!(s_g1.is_bi_complete());
}

#[test]
fn test_multiple_gateways_query_routing_r2r_downstream() {
    try_init_tracing_subscriber();

    let g0 = HarnessBuilder::new()
        .mode(WhatAmI::Router)
        .subregions([Region::default_south(WhatAmI::Router)])
        .build();
    let g1 = HarnessBuilder::new()
        .mode(WhatAmI::Router)
        .subregions([Region::default_south(WhatAmI::Router)])
        .build();
    let n = HarnessBuilder::new()
        .mode(WhatAmI::Router)
        .subregions([Region::Local])
        .build();
    let s = HarnessBuilder::new()
        .mode(WhatAmI::Router)
        .subregions([Region::Local])
        .build();

    let ss = s.new_session();
    let ns = n.new_session();

    let mut n_g0 = Connection {
        a: &n,
        a2b: FaceDef::default().mode(WhatAmI::Router),
        b: &g0,
        b2a: FaceDef::default().mode(WhatAmI::Router),
    }
    .establish();

    let mut n_g1 = Connection {
        a: &n,
        a2b: FaceDef::default().mode(WhatAmI::Router),
        b: &g1,
        b2a: FaceDef::default().mode(WhatAmI::Router),
    }
    .establish();

    let mut s_g0 = Connection {
        a: &s,
        a2b: FaceDef::default()
            .mode(WhatAmI::Router)
            .remote_bound(Bound::South),
        b: &g0,
        b2a: FaceDef::default()
            .mode(WhatAmI::Router)
            .region(Region::default_south(WhatAmI::Router)),
    }
    .establish();

    let mut s_g1 = Connection {
        a: &s,
        a2b: FaceDef::default()
            .mode(WhatAmI::Router)
            .remote_bound(Bound::South),
        b: &g1,
        b2a: FaceDef::default()
            .mode(WhatAmI::Router)
            .region(Region::default_south(WhatAmI::Router)),
    }
    .establish();

    let mut bi_fwd_all = || {
        EstablishedConnection::bi_fwd_many_unbounded([&mut n_g0, &mut n_g1, &mut s_g0, &mut s_g1])
    };

    bi_fwd_all();

    let ke = "k";

    ss.declare_queryable(None, 1, ke, QueryableInfoType::DEFAULT);
    bi_fwd_all();

    ns.query(1, ke);
    bi_fwd_all();

    assert_eq!(ss.recorder().requests().len(), 1);

    assert!(n_g0.is_bi_complete());
    assert!(n_g1.is_bi_complete());
    assert!(s_g0.is_bi_complete());
    assert!(s_g1.is_bi_complete());
}

#[test]
fn test_multiple_gateways_data_routing_p2r_upstream() {
    try_init_tracing_subscriber();

    let g0 = HarnessBuilder::new()
        .mode(WhatAmI::Router)
        .subregions([Region::default_south(WhatAmI::Peer)])
        .build();
    let g1 = HarnessBuilder::new()
        .mode(WhatAmI::Router)
        .subregions([Region::default_south(WhatAmI::Peer)])
        .build();
    let r = HarnessBuilder::new()
        .mode(WhatAmI::Router)
        .subregions([Region::Local])
        .build();
    let p = HarnessBuilder::new()
        .mode(WhatAmI::Peer)
        .subregions([Region::Local])
        .build();

    let ps = p.new_session();
    let rs = r.new_session();

    let mut r_g0 = Connection {
        a: &r,
        a2b: FaceDef::default().mode(WhatAmI::Router),
        b: &g0,
        b2a: FaceDef::default().mode(WhatAmI::Router),
    }
    .establish();

    let mut r_g1 = Connection {
        a: &r,
        a2b: FaceDef::default().mode(WhatAmI::Router),
        b: &g1,
        b2a: FaceDef::default().mode(WhatAmI::Router),
    }
    .establish();

    let mut p_g0 = Connection {
        a: &p,
        a2b: FaceDef::default()
            .mode(WhatAmI::Router)
            .remote_bound(Bound::South),
        b: &g0,
        b2a: FaceDef::default()
            .mode(WhatAmI::Peer)
            .region(Region::default_south(WhatAmI::Peer)),
    }
    .establish();

    let mut p_g1 = Connection {
        a: &p,
        a2b: FaceDef::default()
            .mode(WhatAmI::Router)
            .remote_bound(Bound::South),
        b: &g1,
        b2a: FaceDef::default()
            .mode(WhatAmI::Peer)
            .region(Region::default_south(WhatAmI::Peer)),
    }
    .establish();

    let mut bi_fwd_all = || {
        EstablishedConnection::bi_fwd_many_unbounded([&mut r_g0, &mut r_g1, &mut p_g0, &mut p_g1])
    };

    bi_fwd_all();

    let ke = "k";

    rs.declare_subscriber(None, 1, ke);
    bi_fwd_all();

    // The peer routes upstream unconditionally without having sent an interest.
    ps.put(ke, vec![42]);
    bi_fwd_all();

    assert_eq!(rs.recorder().pushes().len(), 1);

    assert!(r_g0.is_bi_complete());
    assert!(r_g1.is_bi_complete());
    assert!(p_g0.is_bi_complete());
    assert!(p_g1.is_bi_complete());
}

#[test]
fn test_multiple_gateways_data_routing_p2r_upstream_with_interest() {
    try_init_tracing_subscriber();

    let g0 = HarnessBuilder::new()
        .mode(WhatAmI::Router)
        .subregions([Region::default_south(WhatAmI::Peer)])
        .build();
    let g1 = HarnessBuilder::new()
        .mode(WhatAmI::Router)
        .subregions([Region::default_south(WhatAmI::Peer)])
        .build();
    let r = HarnessBuilder::new()
        .mode(WhatAmI::Router)
        .subregions([Region::Local])
        .build();
    let p = HarnessBuilder::new()
        .mode(WhatAmI::Peer)
        .subregions([Region::Local])
        .build();

    let ps = p.new_session();
    let rs = r.new_session();

    let mut r_g0 = Connection {
        a: &r,
        a2b: FaceDef::default().mode(WhatAmI::Router),
        b: &g0,
        b2a: FaceDef::default().mode(WhatAmI::Router),
    }
    .establish();

    let mut r_g1 = Connection {
        a: &r,
        a2b: FaceDef::default().mode(WhatAmI::Router),
        b: &g1,
        b2a: FaceDef::default().mode(WhatAmI::Router),
    }
    .establish();

    let mut p_g0 = Connection {
        a: &p,
        a2b: FaceDef::default()
            .mode(WhatAmI::Router)
            .remote_bound(Bound::South),
        b: &g0,
        b2a: FaceDef::default()
            .mode(WhatAmI::Peer)
            .region(Region::default_south(WhatAmI::Peer)),
    }
    .establish();

    let mut p_g1 = Connection {
        a: &p,
        a2b: FaceDef::default()
            .mode(WhatAmI::Router)
            .remote_bound(Bound::South),
        b: &g1,
        b2a: FaceDef::default()
            .mode(WhatAmI::Peer)
            .region(Region::default_south(WhatAmI::Peer)),
    }
    .establish();

    let mut bi_fwd_all = || {
        EstablishedConnection::bi_fwd_many_unbounded([&mut r_g0, &mut r_g1, &mut p_g0, &mut p_g1])
    };

    bi_fwd_all();

    let ke = "k";

    rs.declare_subscriber(None, 1, ke);
    bi_fwd_all();

    ps.interest(
        1,
        InterestMode::CurrentFuture,
        InterestOptions::SUBSCRIBERS,
        ke,
    );
    bi_fwd_all();

    ps.put(ke, vec![42]);
    bi_fwd_all();

    assert_eq!(rs.recorder().pushes().len(), 1);

    assert!(r_g0.is_bi_complete());
    assert!(r_g1.is_bi_complete());
    assert!(p_g0.is_bi_complete());
    assert!(p_g1.is_bi_complete());
}

#[test]
fn test_multiple_gateways_query_routing_p2r_upstream() {
    try_init_tracing_subscriber();

    let g0 = HarnessBuilder::new()
        .mode(WhatAmI::Router)
        .subregions([Region::default_south(WhatAmI::Peer)])
        .build();
    let g1 = HarnessBuilder::new()
        .mode(WhatAmI::Router)
        .subregions([Region::default_south(WhatAmI::Peer)])
        .build();
    let r = HarnessBuilder::new()
        .mode(WhatAmI::Router)
        .subregions([Region::Local])
        .build();
    let p = HarnessBuilder::new()
        .mode(WhatAmI::Peer)
        .subregions([Region::Local])
        .build();

    let ps = p.new_session();
    let rs = r.new_session();

    let mut r_g0 = Connection {
        a: &r,
        a2b: FaceDef::default().mode(WhatAmI::Router),
        b: &g0,
        b2a: FaceDef::default().mode(WhatAmI::Router),
    }
    .establish();

    let mut r_g1 = Connection {
        a: &r,
        a2b: FaceDef::default().mode(WhatAmI::Router),
        b: &g1,
        b2a: FaceDef::default().mode(WhatAmI::Router),
    }
    .establish();

    let mut p_g0 = Connection {
        a: &p,
        a2b: FaceDef::default()
            .mode(WhatAmI::Router)
            .remote_bound(Bound::South),
        b: &g0,
        b2a: FaceDef::default()
            .mode(WhatAmI::Peer)
            .region(Region::default_south(WhatAmI::Peer)),
    }
    .establish();

    let mut p_g1 = Connection {
        a: &p,
        a2b: FaceDef::default()
            .mode(WhatAmI::Router)
            .remote_bound(Bound::South),
        b: &g1,
        b2a: FaceDef::default()
            .mode(WhatAmI::Peer)
            .region(Region::default_south(WhatAmI::Peer)),
    }
    .establish();

    let mut bi_fwd_all = || {
        EstablishedConnection::bi_fwd_many_unbounded([&mut r_g0, &mut r_g1, &mut p_g0, &mut p_g1])
    };

    bi_fwd_all();

    let ke = "k";

    rs.declare_queryable(None, 1, ke, QueryableInfoType::DEFAULT);
    bi_fwd_all();

    // The peer routes the query upstream unconditionally without having sent an interest.
    ps.query(1, ke);
    bi_fwd_all();

    assert!(r_g0.is_bi_complete());
    assert!(r_g1.is_bi_complete());
    assert!(p_g0.is_bi_complete());
    assert!(p_g1.is_bi_complete());

    assert_eq!(rs.recorder().requests().len(), 1);
}

#[test]
fn test_multiple_gateways_query_routing_p2r_upstream_with_interest() {
    try_init_tracing_subscriber();

    let g0 = HarnessBuilder::new()
        .mode(WhatAmI::Router)
        .subregions([Region::default_south(WhatAmI::Peer)])
        .build();
    let g1 = HarnessBuilder::new()
        .mode(WhatAmI::Router)
        .subregions([Region::default_south(WhatAmI::Peer)])
        .build();
    let r = HarnessBuilder::new()
        .mode(WhatAmI::Router)
        .subregions([Region::Local])
        .build();
    let p = HarnessBuilder::new()
        .mode(WhatAmI::Peer)
        .subregions([Region::Local])
        .build();

    let ps = p.new_session();
    let rs = r.new_session();

    let mut r_g0 = Connection {
        a: &r,
        a2b: FaceDef::default().mode(WhatAmI::Router),
        b: &g0,
        b2a: FaceDef::default().mode(WhatAmI::Router),
    }
    .establish();

    let mut r_g1 = Connection {
        a: &r,
        a2b: FaceDef::default().mode(WhatAmI::Router),
        b: &g1,
        b2a: FaceDef::default().mode(WhatAmI::Router),
    }
    .establish();

    let mut p_g0 = Connection {
        a: &p,
        a2b: FaceDef::default()
            .mode(WhatAmI::Router)
            .remote_bound(Bound::South),
        b: &g0,
        b2a: FaceDef::default()
            .mode(WhatAmI::Peer)
            .region(Region::default_south(WhatAmI::Peer)),
    }
    .establish();

    let mut p_g1 = Connection {
        a: &p,
        a2b: FaceDef::default()
            .mode(WhatAmI::Router)
            .remote_bound(Bound::South),
        b: &g1,
        b2a: FaceDef::default()
            .mode(WhatAmI::Peer)
            .region(Region::default_south(WhatAmI::Peer)),
    }
    .establish();

    let mut bi_fwd_all = || {
        EstablishedConnection::bi_fwd_many_unbounded([&mut r_g0, &mut r_g1, &mut p_g0, &mut p_g1])
    };

    bi_fwd_all();

    let ke = "k";

    rs.declare_queryable(None, 1, ke, QueryableInfoType::DEFAULT);
    bi_fwd_all();

    ps.interest(
        1,
        InterestMode::CurrentFuture,
        InterestOptions::QUERYABLES,
        ke,
    );
    bi_fwd_all();

    ps.query(1, ke);
    bi_fwd_all();

    assert_eq!(rs.recorder().requests().len(), 1);

    assert!(r_g0.is_bi_complete());
    assert!(r_g1.is_bi_complete());
    assert!(p_g0.is_bi_complete());
    assert!(p_g1.is_bi_complete());
}

#[test]
fn test_multiple_gateways_data_routing_r2r_upstream() {
    try_init_tracing_subscriber();

    let g0 = HarnessBuilder::new()
        .mode(WhatAmI::Router)
        .subregions([Region::default_south(WhatAmI::Router)])
        .build();
    let g1 = HarnessBuilder::new()
        .mode(WhatAmI::Router)
        .subregions([Region::default_south(WhatAmI::Router)])
        .build();
    let n = HarnessBuilder::new()
        .mode(WhatAmI::Router)
        .subregions([Region::Local])
        .build();
    let s = HarnessBuilder::new()
        .mode(WhatAmI::Router)
        .subregions([Region::Local])
        .build();

    let ns = n.new_session();
    let ss = s.new_session();

    let mut n_g0 = Connection {
        a: &n,
        a2b: FaceDef::default().mode(WhatAmI::Router),
        b: &g0,
        b2a: FaceDef::default().mode(WhatAmI::Router),
    }
    .establish();

    let mut n_g1 = Connection {
        a: &n,
        a2b: FaceDef::default().mode(WhatAmI::Router),
        b: &g1,
        b2a: FaceDef::default().mode(WhatAmI::Router),
    }
    .establish();

    let mut s_g0 = Connection {
        a: &s,
        a2b: FaceDef::default()
            .mode(WhatAmI::Router)
            .remote_bound(Bound::South),
        b: &g0,
        b2a: FaceDef::default()
            .mode(WhatAmI::Router)
            .region(Region::default_south(WhatAmI::Router)),
    }
    .establish();

    let mut s_g1 = Connection {
        a: &s,
        a2b: FaceDef::default()
            .mode(WhatAmI::Router)
            .remote_bound(Bound::South),
        b: &g1,
        b2a: FaceDef::default()
            .mode(WhatAmI::Router)
            .region(Region::default_south(WhatAmI::Router)),
    }
    .establish();

    let mut bi_fwd_all = || {
        EstablishedConnection::bi_fwd_many_unbounded([&mut n_g0, &mut n_g1, &mut s_g0, &mut s_g1])
    };

    bi_fwd_all();

    let ke = "k";

    ns.declare_subscriber(None, 1, ke);
    bi_fwd_all();

    ss.put(ke, vec![0x42]);
    bi_fwd_all();

    assert_eq!(ns.recorder().pushes().len(), 1);

    assert!(n_g0.is_bi_complete());
    assert!(n_g1.is_bi_complete());
    assert!(s_g0.is_bi_complete());
    assert!(s_g1.is_bi_complete());
}

#[test]
fn test_multiple_gateways_query_routing_r2r_upstream() {
    try_init_tracing_subscriber();

    let g0 = HarnessBuilder::new()
        .mode(WhatAmI::Router)
        .subregions([Region::default_south(WhatAmI::Router)])
        .build();
    let g1 = HarnessBuilder::new()
        .mode(WhatAmI::Router)
        .subregions([Region::default_south(WhatAmI::Router)])
        .build();
    let n = HarnessBuilder::new()
        .mode(WhatAmI::Router)
        .subregions([Region::Local])
        .build();
    let s = HarnessBuilder::new()
        .mode(WhatAmI::Router)
        .subregions([Region::Local])
        .build();

    let ns = n.new_session();
    let ss = s.new_session();

    let mut n_g0 = Connection {
        a: &n,
        a2b: FaceDef::default().mode(WhatAmI::Router),
        b: &g0,
        b2a: FaceDef::default().mode(WhatAmI::Router),
    }
    .establish();

    let mut n_g1 = Connection {
        a: &n,
        a2b: FaceDef::default().mode(WhatAmI::Router),
        b: &g1,
        b2a: FaceDef::default().mode(WhatAmI::Router),
    }
    .establish();

    let mut s_g0 = Connection {
        a: &s,
        a2b: FaceDef::default()
            .mode(WhatAmI::Router)
            .remote_bound(Bound::South),
        b: &g0,
        b2a: FaceDef::default()
            .mode(WhatAmI::Router)
            .region(Region::default_south(WhatAmI::Router)),
    }
    .establish();

    let mut s_g1 = Connection {
        a: &s,
        a2b: FaceDef::default()
            .mode(WhatAmI::Router)
            .remote_bound(Bound::South),
        b: &g1,
        b2a: FaceDef::default()
            .mode(WhatAmI::Router)
            .region(Region::default_south(WhatAmI::Router)),
    }
    .establish();

    let mut bi_fwd_all = || {
        EstablishedConnection::bi_fwd_many_unbounded([&mut n_g0, &mut n_g1, &mut s_g0, &mut s_g1])
    };

    bi_fwd_all();

    let ke = "k";

    ns.declare_queryable(None, 1, ke, QueryableInfoType::DEFAULT);
    bi_fwd_all();

    ss.query(1, ke);
    bi_fwd_all();

    assert!(n_g0.is_bi_complete());
    assert!(n_g1.is_bi_complete());
    assert!(s_g0.is_bi_complete());
    assert!(s_g1.is_bi_complete());

    assert_eq!(ns.recorder().requests().len(), 1);
}

#[test]
fn test_multiple_gateways_data_routing_p2p_downstream() {
    try_init_tracing_subscriber();

    let g0 = HarnessBuilder::new()
        .mode(WhatAmI::Peer)
        .subregions([Region::default_south(WhatAmI::Peer)])
        .build();
    let g1 = HarnessBuilder::new()
        .mode(WhatAmI::Peer)
        .subregions([Region::default_south(WhatAmI::Peer)])
        .build();
    let n = HarnessBuilder::new()
        .mode(WhatAmI::Peer)
        .subregions([Region::Local])
        .build();
    let s = HarnessBuilder::new()
        .mode(WhatAmI::Peer)
        .subregions([Region::Local])
        .build();

    let ss = s.new_session();
    let ns = n.new_session();

    let mut n_g0 = Connection {
        a: &n,
        a2b: FaceDef::default().mode(WhatAmI::Peer),
        b: &g0,
        b2a: FaceDef::default().mode(WhatAmI::Peer),
    }
    .establish();

    let mut n_g1 = Connection {
        a: &n,
        a2b: FaceDef::default().mode(WhatAmI::Peer),
        b: &g1,
        b2a: FaceDef::default().mode(WhatAmI::Peer),
    }
    .establish();

    let mut s_g0 = Connection {
        a: &s,
        a2b: FaceDef::default()
            .mode(WhatAmI::Peer)
            .remote_bound(Bound::South),
        b: &g0,
        b2a: FaceDef::default()
            .mode(WhatAmI::Peer)
            .region(Region::default_south(WhatAmI::Peer)),
    }
    .establish();

    let mut s_g1 = Connection {
        a: &s,
        a2b: FaceDef::default()
            .mode(WhatAmI::Peer)
            .remote_bound(Bound::South),
        b: &g1,
        b2a: FaceDef::default()
            .mode(WhatAmI::Peer)
            .region(Region::default_south(WhatAmI::Peer)),
    }
    .establish();

    let mut g0_g1 = Connection {
        a: &g0,
        a2b: FaceDef::default().mode(WhatAmI::Peer),
        b: &g1,
        b2a: FaceDef::default().mode(WhatAmI::Peer),
    }
    .establish();

    let mut bi_fwd_all = || {
        EstablishedConnection::bi_fwd_many_unbounded([
            &mut n_g0, &mut n_g1, &mut s_g0, &mut s_g1, &mut g0_g1,
        ])
    };

    bi_fwd_all();

    let ke = "k";

    ss.declare_subscriber(None, 1, ke);
    bi_fwd_all();

    ns.put(ke, vec![0x42]);
    bi_fwd_all();

    assert_eq!(ss.recorder().pushes().len(), 1);

    assert!(n_g0.is_bi_complete());
    assert!(n_g1.is_bi_complete());
    assert!(s_g0.is_bi_complete());
    assert!(s_g1.is_bi_complete());
    assert!(g0_g1.is_bi_complete());
}

#[test]
fn test_multiple_gateways_query_routing_p2p_downstream() {
    try_init_tracing_subscriber();

    let g0 = HarnessBuilder::new()
        .mode(WhatAmI::Peer)
        .subregions([Region::default_south(WhatAmI::Peer)])
        .build();
    let g1 = HarnessBuilder::new()
        .mode(WhatAmI::Peer)
        .subregions([Region::default_south(WhatAmI::Peer)])
        .build();
    let n = HarnessBuilder::new()
        .mode(WhatAmI::Peer)
        .subregions([Region::Local])
        .build();
    let s = HarnessBuilder::new()
        .mode(WhatAmI::Peer)
        .subregions([Region::Local])
        .build();

    let ss = s.new_session();
    let ns = n.new_session();

    let mut n_g0 = Connection {
        a: &n,
        a2b: FaceDef::default().mode(WhatAmI::Peer),
        b: &g0,
        b2a: FaceDef::default().mode(WhatAmI::Peer),
    }
    .establish();

    let mut n_g1 = Connection {
        a: &n,
        a2b: FaceDef::default().mode(WhatAmI::Peer),
        b: &g1,
        b2a: FaceDef::default().mode(WhatAmI::Peer),
    }
    .establish();

    let mut s_g0 = Connection {
        a: &s,
        a2b: FaceDef::default()
            .mode(WhatAmI::Peer)
            .remote_bound(Bound::South),
        b: &g0,
        b2a: FaceDef::default()
            .mode(WhatAmI::Peer)
            .region(Region::default_south(WhatAmI::Peer)),
    }
    .establish();

    let mut s_g1 = Connection {
        a: &s,
        a2b: FaceDef::default()
            .mode(WhatAmI::Peer)
            .remote_bound(Bound::South),
        b: &g1,
        b2a: FaceDef::default()
            .mode(WhatAmI::Peer)
            .region(Region::default_south(WhatAmI::Peer)),
    }
    .establish();

    let mut g0_g1 = Connection {
        a: &g0,
        a2b: FaceDef::default().mode(WhatAmI::Peer),
        b: &g1,
        b2a: FaceDef::default().mode(WhatAmI::Peer),
    }
    .establish();

    let mut bi_fwd_all = || {
        EstablishedConnection::bi_fwd_many_unbounded([
            &mut n_g0, &mut n_g1, &mut s_g0, &mut s_g1, &mut g0_g1,
        ])
    };

    bi_fwd_all();

    let ke = "k";

    ss.declare_queryable(None, 1, ke, QueryableInfoType::DEFAULT);
    bi_fwd_all();

    ns.query(1, ke);
    bi_fwd_all();

    assert_eq!(ss.recorder().requests().len(), 1);

    assert!(n_g0.is_bi_complete());
    assert!(n_g1.is_bi_complete());
    assert!(s_g0.is_bi_complete());
    assert!(s_g1.is_bi_complete());
    assert!(g0_g1.is_bi_complete());
}

#[test]
fn test_multiple_gateways_data_routing_p2p_upstream() {
    try_init_tracing_subscriber();

    let g0 = HarnessBuilder::new()
        .mode(WhatAmI::Peer)
        .subregions([Region::default_south(WhatAmI::Peer)])
        .build();
    let g1 = HarnessBuilder::new()
        .mode(WhatAmI::Peer)
        .subregions([Region::default_south(WhatAmI::Peer)])
        .build();
    let n = HarnessBuilder::new()
        .mode(WhatAmI::Peer)
        .subregions([Region::Local])
        .build();
    let s = HarnessBuilder::new()
        .mode(WhatAmI::Peer)
        .subregions([Region::Local])
        .build();

    let ns = n.new_session();
    let ss = s.new_session();

    let mut n_g0 = Connection {
        a: &n,
        a2b: FaceDef::default().mode(WhatAmI::Peer),
        b: &g0,
        b2a: FaceDef::default().mode(WhatAmI::Peer),
    }
    .establish();

    let mut n_g1 = Connection {
        a: &n,
        a2b: FaceDef::default().mode(WhatAmI::Peer),
        b: &g1,
        b2a: FaceDef::default().mode(WhatAmI::Peer),
    }
    .establish();

    let mut s_g0 = Connection {
        a: &s,
        a2b: FaceDef::default()
            .mode(WhatAmI::Peer)
            .remote_bound(Bound::South),
        b: &g0,
        b2a: FaceDef::default()
            .mode(WhatAmI::Peer)
            .region(Region::default_south(WhatAmI::Peer)),
    }
    .establish();

    let mut s_g1 = Connection {
        a: &s,
        a2b: FaceDef::default()
            .mode(WhatAmI::Peer)
            .remote_bound(Bound::South),
        b: &g1,
        b2a: FaceDef::default()
            .mode(WhatAmI::Peer)
            .region(Region::default_south(WhatAmI::Peer)),
    }
    .establish();

    let mut g0_g1 = Connection {
        a: &g0,
        a2b: FaceDef::default().mode(WhatAmI::Peer),
        b: &g1,
        b2a: FaceDef::default().mode(WhatAmI::Peer),
    }
    .establish();

    let mut bi_fwd_all = || {
        EstablishedConnection::bi_fwd_many_unbounded([
            &mut n_g0, &mut n_g1, &mut s_g0, &mut s_g1, &mut g0_g1,
        ])
    };

    bi_fwd_all();

    let ke = "k";

    ns.declare_subscriber(None, 1, ke);
    bi_fwd_all();

    ss.put(ke, vec![0x42]);
    bi_fwd_all();

    assert!(n_g0.is_bi_complete());
    assert!(n_g1.is_bi_complete());
    assert!(s_g0.is_bi_complete());
    assert!(s_g1.is_bi_complete());
    assert!(g0_g1.is_bi_complete());

    assert_eq!(ns.recorder().pushes().len(), 1);
}

#[test]
fn test_multiple_gateways_data_routing_p2p_upstream_with_interest() {
    try_init_tracing_subscriber();

    let g0 = HarnessBuilder::new()
        .mode(WhatAmI::Peer)
        .subregions([Region::default_south(WhatAmI::Peer)])
        .build();
    let g1 = HarnessBuilder::new()
        .mode(WhatAmI::Peer)
        .subregions([Region::default_south(WhatAmI::Peer)])
        .build();
    let n = HarnessBuilder::new()
        .mode(WhatAmI::Peer)
        .subregions([Region::Local])
        .build();
    let s = HarnessBuilder::new()
        .mode(WhatAmI::Peer)
        .subregions([Region::Local])
        .build();

    let ns = n.new_session();
    let ss = s.new_session();

    let mut n_g0 = Connection {
        a: &n,
        a2b: FaceDef::default().mode(WhatAmI::Peer),
        b: &g0,
        b2a: FaceDef::default().mode(WhatAmI::Peer),
    }
    .establish();

    let mut n_g1 = Connection {
        a: &n,
        a2b: FaceDef::default().mode(WhatAmI::Peer),
        b: &g1,
        b2a: FaceDef::default().mode(WhatAmI::Peer),
    }
    .establish();

    let mut s_g0 = Connection {
        a: &s,
        a2b: FaceDef::default()
            .mode(WhatAmI::Peer)
            .remote_bound(Bound::South),
        b: &g0,
        b2a: FaceDef::default()
            .mode(WhatAmI::Peer)
            .region(Region::default_south(WhatAmI::Peer)),
    }
    .establish();

    let mut s_g1 = Connection {
        a: &s,
        a2b: FaceDef::default()
            .mode(WhatAmI::Peer)
            .remote_bound(Bound::South),
        b: &g1,
        b2a: FaceDef::default()
            .mode(WhatAmI::Peer)
            .region(Region::default_south(WhatAmI::Peer)),
    }
    .establish();

    let mut g0_g1 = Connection {
        a: &g0,
        a2b: FaceDef::default().mode(WhatAmI::Peer),
        b: &g1,
        b2a: FaceDef::default().mode(WhatAmI::Peer),
    }
    .establish();

    let mut bi_fwd_all = || {
        EstablishedConnection::bi_fwd_many_unbounded([
            &mut n_g0, &mut n_g1, &mut s_g0, &mut s_g1, &mut g0_g1,
        ])
    };

    bi_fwd_all();

    let ke = "k";

    ns.declare_subscriber(None, 1, ke);
    bi_fwd_all();

    ss.interest(
        1,
        InterestMode::CurrentFuture,
        InterestOptions::SUBSCRIBERS,
        ke,
    );
    bi_fwd_all();

    ss.put(ke, vec![0x42]);
    bi_fwd_all();

    assert_eq!(ns.recorder().pushes().len(), 1);

    assert!(n_g0.is_bi_complete());
    assert!(n_g1.is_bi_complete());
    assert!(s_g0.is_bi_complete());
    assert!(s_g1.is_bi_complete());
    assert!(g0_g1.is_bi_complete());
}

#[test]
fn test_multiple_gateways_query_routing_p2p_upstream() {
    try_init_tracing_subscriber();

    let g0 = HarnessBuilder::new()
        .mode(WhatAmI::Peer)
        .subregions([Region::default_south(WhatAmI::Peer)])
        .build();
    let g1 = HarnessBuilder::new()
        .mode(WhatAmI::Peer)
        .subregions([Region::default_south(WhatAmI::Peer)])
        .build();
    let n = HarnessBuilder::new()
        .mode(WhatAmI::Peer)
        .subregions([Region::Local])
        .build();
    let s = HarnessBuilder::new()
        .mode(WhatAmI::Peer)
        .subregions([Region::Local])
        .build();

    let ns = n.new_session();
    let ss = s.new_session();

    let mut n_g0 = Connection {
        a: &n,
        a2b: FaceDef::default().mode(WhatAmI::Peer),
        b: &g0,
        b2a: FaceDef::default().mode(WhatAmI::Peer),
    }
    .establish();

    let mut n_g1 = Connection {
        a: &n,
        a2b: FaceDef::default().mode(WhatAmI::Peer),
        b: &g1,
        b2a: FaceDef::default().mode(WhatAmI::Peer),
    }
    .establish();

    let mut s_g0 = Connection {
        a: &s,
        a2b: FaceDef::default()
            .mode(WhatAmI::Peer)
            .remote_bound(Bound::South),
        b: &g0,
        b2a: FaceDef::default()
            .mode(WhatAmI::Peer)
            .region(Region::default_south(WhatAmI::Peer)),
    }
    .establish();

    let mut s_g1 = Connection {
        a: &s,
        a2b: FaceDef::default()
            .mode(WhatAmI::Peer)
            .remote_bound(Bound::South),
        b: &g1,
        b2a: FaceDef::default()
            .mode(WhatAmI::Peer)
            .region(Region::default_south(WhatAmI::Peer)),
    }
    .establish();

    let mut g0_g1 = Connection {
        a: &g0,
        a2b: FaceDef::default().mode(WhatAmI::Peer),
        b: &g1,
        b2a: FaceDef::default().mode(WhatAmI::Peer),
    }
    .establish();

    let mut bi_fwd_all = || {
        EstablishedConnection::bi_fwd_many_unbounded([
            &mut n_g0, &mut n_g1, &mut s_g0, &mut s_g1, &mut g0_g1,
        ])
    };

    bi_fwd_all();

    let ke = "k";

    ns.declare_queryable(None, 1, ke, QueryableInfoType::DEFAULT);
    bi_fwd_all();

    ss.query(1, ke);
    bi_fwd_all();

    assert_eq!(ns.recorder().requests().len(), 1);

    assert!(n_g0.is_bi_complete());
    assert!(n_g1.is_bi_complete());
    assert!(s_g0.is_bi_complete());
    assert!(s_g1.is_bi_complete());
    assert!(g0_g1.is_bi_complete());
}

#[test]
fn test_multiple_gateways_query_routing_p2p_upstream_with_interest() {
    try_init_tracing_subscriber();

    let g0 = HarnessBuilder::new()
        .mode(WhatAmI::Peer)
        .subregions([Region::default_south(WhatAmI::Peer)])
        .build();
    let g1 = HarnessBuilder::new()
        .mode(WhatAmI::Peer)
        .subregions([Region::default_south(WhatAmI::Peer)])
        .build();
    let n = HarnessBuilder::new()
        .mode(WhatAmI::Peer)
        .subregions([Region::Local])
        .build();
    let s = HarnessBuilder::new()
        .mode(WhatAmI::Peer)
        .subregions([Region::Local])
        .build();

    let ns = n.new_session();
    let ss = s.new_session();

    let mut n_g0 = Connection {
        a: &n,
        a2b: FaceDef::default().mode(WhatAmI::Peer),
        b: &g0,
        b2a: FaceDef::default().mode(WhatAmI::Peer),
    }
    .establish();

    let mut n_g1 = Connection {
        a: &n,
        a2b: FaceDef::default().mode(WhatAmI::Peer),
        b: &g1,
        b2a: FaceDef::default().mode(WhatAmI::Peer),
    }
    .establish();

    let mut s_g0 = Connection {
        a: &s,
        a2b: FaceDef::default()
            .mode(WhatAmI::Peer)
            .remote_bound(Bound::South),
        b: &g0,
        b2a: FaceDef::default()
            .mode(WhatAmI::Peer)
            .region(Region::default_south(WhatAmI::Peer)),
    }
    .establish();

    let mut s_g1 = Connection {
        a: &s,
        a2b: FaceDef::default()
            .mode(WhatAmI::Peer)
            .remote_bound(Bound::South),
        b: &g1,
        b2a: FaceDef::default()
            .mode(WhatAmI::Peer)
            .region(Region::default_south(WhatAmI::Peer)),
    }
    .establish();

    let mut g0_g1 = Connection {
        a: &g0,
        a2b: FaceDef::default().mode(WhatAmI::Peer),
        b: &g1,
        b2a: FaceDef::default().mode(WhatAmI::Peer),
    }
    .establish();

    let mut bi_fwd_all = || {
        EstablishedConnection::bi_fwd_many_unbounded([
            &mut n_g0, &mut n_g1, &mut s_g0, &mut s_g1, &mut g0_g1,
        ])
    };

    bi_fwd_all();

    let ke = "k";

    ns.declare_queryable(None, 1, ke, QueryableInfoType::DEFAULT);
    bi_fwd_all();

    ss.interest(
        1,
        InterestMode::CurrentFuture,
        InterestOptions::QUERYABLES,
        ke,
    );
    bi_fwd_all();

    ss.query(1, ke);
    bi_fwd_all();

    assert_eq!(ns.recorder().requests().len(), 1);

    assert!(n_g0.is_bi_complete());
    assert!(n_g1.is_bi_complete());
    assert!(s_g0.is_bi_complete());
    assert!(s_g1.is_bi_complete());
    assert!(g0_g1.is_bi_complete());
}

/// Regression test for `inter_region_filter` using `src_zid` instead of `fwd_zid` when
/// `src.bound() == South`.
///
/// ```text
///           n            ← subscriber ns
///         /   \
///       g1     g2        ← two router gateways, each with a South{Router} sub-region
///         \   /
///           m            ← intermediate south-region router (Region::Local)
/// ```
#[test]
fn test_multiple_gateways_data_routing_r2r_upstream_gateway_source() {
    try_init_tracing_subscriber();

    // g1 needs Region::Local so it can host a session (g1s).
    let g1 = HarnessBuilder::new()
        .mode(WhatAmI::Router)
        .subregions([Region::default_south(WhatAmI::Router), Region::Local])
        .build();
    let g2 = HarnessBuilder::new()
        .mode(WhatAmI::Router)
        .subregions([Region::default_south(WhatAmI::Router)])
        .build();
    let n = HarnessBuilder::new()
        .mode(WhatAmI::Router)
        .subregions([Region::Local])
        .build();
    let m = HarnessBuilder::new()
        .mode(WhatAmI::Router)
        .subregions([Region::Local])
        .build();

    let ns = n.new_session();
    let g1s = g1.new_session();

    // North side: n connects to both gateways.
    let mut n_g1 = Connection {
        a: &n,
        a2b: FaceDef::default().mode(WhatAmI::Router),
        b: &g1,
        b2a: FaceDef::default().mode(WhatAmI::Router),
    }
    .establish();

    let mut n_g2 = Connection {
        a: &n,
        a2b: FaceDef::default().mode(WhatAmI::Router),
        b: &g2,
        b2a: FaceDef::default().mode(WhatAmI::Router),
    }
    .establish();

    // South side: m connects to both gateways, bridging their south sub-regions.
    let mut m_g1 = Connection {
        a: &m,
        a2b: FaceDef::default()
            .mode(WhatAmI::Router)
            .remote_bound(Bound::South),
        b: &g1,
        b2a: FaceDef::default()
            .mode(WhatAmI::Router)
            .region(Region::default_south(WhatAmI::Router)),
    }
    .establish();

    let mut m_g2 = Connection {
        a: &m,
        a2b: FaceDef::default()
            .mode(WhatAmI::Router)
            .remote_bound(Bound::South),
        b: &g2,
        b2a: FaceDef::default()
            .mode(WhatAmI::Router)
            .region(Region::default_south(WhatAmI::Router)),
    }
    .establish();

    let mut bi_fwd_all = || {
        EstablishedConnection::bi_fwd_many_unbounded([&mut n_g1, &mut n_g2, &mut m_g1, &mut m_g2])
    };

    bi_fwd_all();

    let ke = "k";

    ns.declare_subscriber(None, 1, ke);
    bi_fwd_all();

    g1s.put(ke, vec![0x42]);
    bi_fwd_all();

    assert_eq!(ns.recorder().pushes().len(), 1);

    assert!(n_g1.is_bi_complete());
    assert!(n_g2.is_bi_complete());
    assert!(m_g1.is_bi_complete());
    assert!(m_g2.is_bi_complete());
}

/// Same as [`multiple_gateways_data_routing_r2r_upstream_gateway_source`] but for queries.
#[test]
fn test_multiple_gateways_query_routing_r2r_upstream_gateway_source() {
    try_init_tracing_subscriber();

    let g1 = HarnessBuilder::new()
        .mode(WhatAmI::Router)
        .subregions([Region::default_south(WhatAmI::Router), Region::Local])
        .build();
    let g2 = HarnessBuilder::new()
        .mode(WhatAmI::Router)
        .subregions([Region::default_south(WhatAmI::Router)])
        .build();
    let n = HarnessBuilder::new()
        .mode(WhatAmI::Router)
        .subregions([Region::Local])
        .build();
    let m = HarnessBuilder::new()
        .mode(WhatAmI::Router)
        .subregions([Region::Local])
        .build();

    let ns = n.new_session();
    let g1s = g1.new_session();

    let mut n_g1 = Connection {
        a: &n,
        a2b: FaceDef::default().mode(WhatAmI::Router),
        b: &g1,
        b2a: FaceDef::default().mode(WhatAmI::Router),
    }
    .establish();

    let mut n_g2 = Connection {
        a: &n,
        a2b: FaceDef::default().mode(WhatAmI::Router),
        b: &g2,
        b2a: FaceDef::default().mode(WhatAmI::Router),
    }
    .establish();

    let mut m_g1 = Connection {
        a: &m,
        a2b: FaceDef::default()
            .mode(WhatAmI::Router)
            .remote_bound(Bound::South),
        b: &g1,
        b2a: FaceDef::default()
            .mode(WhatAmI::Router)
            .region(Region::default_south(WhatAmI::Router)),
    }
    .establish();

    let mut m_g2 = Connection {
        a: &m,
        a2b: FaceDef::default()
            .mode(WhatAmI::Router)
            .remote_bound(Bound::South),
        b: &g2,
        b2a: FaceDef::default()
            .mode(WhatAmI::Router)
            .region(Region::default_south(WhatAmI::Router)),
    }
    .establish();

    let mut bi_fwd_all = || {
        EstablishedConnection::bi_fwd_many_unbounded([&mut n_g1, &mut n_g2, &mut m_g1, &mut m_g2])
    };

    bi_fwd_all();

    let ke = "k";

    ns.declare_queryable(None, 1, ke, QueryableInfoType::DEFAULT);
    bi_fwd_all();

    g1s.query(1, ke);
    bi_fwd_all();

    assert_eq!(ns.recorder().requests().len(), 1);

    assert!(n_g1.is_bi_complete());
    assert!(n_g2.is_bi_complete());
    assert!(m_g1.is_bi_complete());
    assert!(m_g2.is_bi_complete());
}

/// In [route_data], we use [send_push_consume] consume to decide whether we may modify the source
/// message in place for forwarding or not. This test ensures that the `consume` API is respected.
///
/// [route_data]: crate::net::routing::dispatcher::pubsub::route_data
/// [send_push_consume]: crate::net::routing::dispatcher::face::Face
#[test]
fn test_push_message_consumption() {
    let g = HarnessBuilder::new()
        .mode(WhatAmI::Client) // Avoid the need for a runtime
        .subregions([Region::Local])
        .start_runtime(false)
        .build();
    let s1 = g.new_session();
    let s2 = g.new_session();

    s2.declare_subscriber(None, 3, "k/*");
    s2.declare_keyexpr(None, 5, "k/0");

    let mut push = Push::from(vec![42]);
    push.wire_expr = "k/0".into();
    let push_clone = push.clone();

    // Here the wire expr is non-scoped
    s1.face
        .send_push_consume(&mut push, Reliability::default(), false);

    assert_eq!(
        s2.recorder().pushes().len(),
        1,
        "s2 should receive exactly one publication"
    );

    assert_eq!(
        s2.recorder().pushes()[0].wire_expr,
        WireExpr {
            scope: 5,
            suffix: "".into(),
            mapping: Mapping::Receiver
        },
        "s2's received publication should be scoped"
    );

    assert_eq!(
        push, push_clone,
        "the push message should not be modified as `consume` is false"
    );
}

/// Test that query routes in router networks are disabled even if queryable info doesn't change upon some queryable undeclaration.
#[test]
fn test_complete_queryable_failover() {
    let r0 = HarnessBuilder::new()
        .mode(WhatAmI::Router)
        .subregions([Region::Local])
        .start_runtime(true)
        .build();

    let r1 = HarnessBuilder::new()
        .mode(WhatAmI::Router)
        .subregions([Region::Local])
        .start_runtime(true)
        .build();

    let r2 = HarnessBuilder::new()
        .mode(WhatAmI::Router)
        .subregions([Region::Local])
        .start_runtime(true)
        .build();

    let mut r0_r1 = Connection {
        a: &r0,
        a2b: FaceDef::default().mode(WhatAmI::Router),
        b: &r1,
        b2a: FaceDef::default().mode(WhatAmI::Router),
    }
    .establish();

    let mut r0_r2 = Connection {
        a: &r0,
        a2b: FaceDef::default().mode(WhatAmI::Router),
        b: &r2,
        b2a: FaceDef::default().mode(WhatAmI::Router),
    }
    .establish();

    let mut bi_fwd_all = || {
        EstablishedConnection::bi_fwd_many_unbounded([&mut r0_r1, &mut r0_r2]);
        assert!(r0_r1.is_bi_complete());
        assert!(r0_r2.is_bi_complete());
    };

    bi_fwd_all();

    let s0 = r0.new_session();
    let s1 = r1.new_session();
    let s2 = r2.new_session();

    let ke = "k";

    s1.declare_queryable(
        None,
        1,
        ke,
        QueryableInfoType {
            complete: true,
            distance: 0,
        },
    );

    s2.declare_queryable(
        None,
        1,
        ke,
        QueryableInfoType {
            complete: true,
            distance: 0,
        },
    );

    bi_fwd_all();

    s0.query(1, ke);

    bi_fwd_all();

    let is_reqs1_empty = s1.recorder().requests().is_empty();
    let is_reqs2_empty = s2.recorder().requests().is_empty();

    assert!(
        is_reqs1_empty || is_reqs2_empty,
        "exactly one queryable should receive the query"
    );

    if !is_reqs1_empty {
        s1.undeclare_queryable(1);
    } else {
        s2.undeclare_queryable(1);
    }

    bi_fwd_all();

    s0.query(2, ke);

    bi_fwd_all();

    assert_eq!(r0_r1.a2b.recorder().requests().len(), 1);
    assert_eq!(r0_r2.a2b.recorder().requests().len(), 1);
}

/// A downstream routing identifier must retain its original pending consumer.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn routed_queries_keep_their_original_consumers_at_identifier_exhaustion() {
    let router = HarnessBuilder::new()
        .mode(WhatAmI::Router)
        .subregions([Region::Local])
        .build();
    let original_caller = router.new_session();
    let second_caller = router.new_session();
    let third_caller = router.new_session();
    let queryable = router.new_session();
    let expression = "query-custody/retained-consumer";
    queryable.declare_queryable(None, 17, expression, QueryableInfoType::DEFAULT);
    let query_request = |id| Request {
        id,
        wire_expr: WireExpr::empty().with_suffix(expression).to_owned(),
        ext_qos: network_ext::QoSType::DEFAULT,
        ext_tstamp: None,
        ext_nodeid: network_ext::NodeIdType::DEFAULT,
        ext_target: QueryTarget::All,
        ext_budget: None,
        ext_timeout: Some(Duration::from_secs(600)),
        ext_ts_stack: None,
        payload: RequestBody::Query(Query::default()),
    };
    let mut original_request = query_request(37);
    let mut second_request = query_request(41);
    let mut third_request = query_request(43);
    original_caller.face.send_request(&mut original_request);
    let original_forwarding = queryable.recorder().requests();
    let [original_forwarded] = original_forwarding.as_slice() else {
        panic!("setup must route the original query to its one real queryable");
    };
    let original_routing_id = original_forwarded.id;
    let (original_query_pointer, original_query, original_timeout) = {
        let queries = router.gateway.tables.queries_lock.read().unwrap();
        let (query, cancellation) = queryable
            .face
            .state
            .pending_queries
            .get(&original_routing_id)
            .expect("the original forwarded query owns its pending registration");
        let originals = (
            Arc::as_ptr(query),
            Arc::downgrade(query),
            cancellation.clone(),
        );
        drop(queries);
        originals
    };
    {
        let queries = router.gateway.tables.queries_lock.write().unwrap();
        let mut state = queryable.face.state.clone();
        // This is the same owning lock required by FaceState::pending_queries.
        get_mut_unchecked(&mut state).next_qid = u32::MAX;
        drop(queries);
    }
    second_caller.face.send_request(&mut second_request);
    third_caller.face.send_request(&mut third_request);
    let routed_requests = queryable.recorder().requests();
    let (retained_query_pointer, retained_query_owner) = {
        let queries = router.gateway.tables.queries_lock.read().unwrap();
        let pointer = queryable
            .face
            .state
            .pending_queries
            .get(&original_routing_id)
            .map(|(query, _)| Arc::as_ptr(query));
        let owner = original_query.upgrade().map(|query| Arc::as_ptr(&query));
        drop(queries);
        (pointer, owner)
    };
    let tags = [
        b"original-query".as_slice(),
        b"second-query",
        b"third-query",
    ];
    let mut original_replies = Vec::new();
    let mut original_finals = Vec::new();
    for (forwarded, tag) in routed_requests.iter().zip(tags) {
        let mut reply = Response {
            rid: forwarded.id,
            wire_expr: WireExpr::empty().with_suffix(expression).to_owned(),
            payload: ResponseBody::Reply(Reply {
                consolidation: ConsolidationMode::None,
                ext_unknown: Vec::new(),
                payload: PushBody::Put(Put {
                    payload: tag.to_vec().into(),
                    ..Put::default()
                }),
            }),
            ext_qos: network_ext::QoSType::DEFAULT,
            ext_tstamp: None,
            ext_respid: None,
            ext_ts_stack: None,
        };
        queryable.face.send_response(&mut reply);
        original_replies.push(reply);
        let mut final_reply = ResponseFinal {
            rid: forwarded.id,
            ext_qos: network_ext::QoSType::DEFAULT,
            ext_tstamp: None,
        };
        queryable.face.send_response_final(&mut final_reply);
        original_finals.push(final_reply);
    }
    // Observe the original token before closing its parent controller, which
    // would otherwise cancel both a replaced orphan and the surviving queries.
    let original_timeout_cancellation = original_timeout.cancelled().now_or_never();
    let original_responses = original_caller.recorder().responses();
    let second_responses = second_caller.recorder().responses();
    let third_responses = third_caller.recorder().responses();
    let original_completion = original_caller.recorder().response_finals();
    let second_completion = second_caller.recorder().response_finals();
    let third_completion = third_caller.recorder().response_finals();
    let mut closed_faces = Vec::new();
    let mut joined_tasks = Vec::new();
    let mut closed_runtime = None;
    let cleanup = tokio::time::timeout(Duration::from_secs(60), async {
        for face in [&queryable, &original_caller, &second_caller, &third_caller] {
            let () = face.face.send_close();
            closed_faces.push(());
            let joined = face.face.state.task_controller.terminate_all_async().await;
            joined_tasks.push(joined);
        }
        let runtime = router
            ._runtime
            .as_ref()
            .expect("the actual router owns its runtime");
        closed_runtime = Some(CloseBuilder::new(runtime).await);
    })
    .await;
    let remaining_queries = {
        let queries = router.gateway.tables.queries_lock.read().unwrap();
        let count = queryable.face.state.pending_queries.len();
        drop(queries);
        count
    };
    // Native inputs, replies, finals, close outcomes and joined controllers all
    // remain owned before the first semantic custody oracle below.
    let Ok(()) = cleanup else {
        panic!("router cleanup did not finish before the query custody oracle");
    };
    assert_eq!(closed_faces, [(), (), (), ()]);
    assert_eq!(joined_tasks, [(), (), (), ()]);
    assert!(matches!(closed_runtime, Some(Ok(()))));
    let [reply] = original_responses.as_slice() else {
        panic!("the original routed query lost its reply consumer at identifier exhaustion");
    };
    assert_eq!(reply.rid, 37);
    let ResponseBody::Reply(Reply {
        consolidation: ConsolidationMode::None,
        ext_unknown,
        payload: PushBody::Put(payload),
    }) = &reply.payload
    else {
        panic!("the original consumer keeps its complete native reply");
    };
    assert_eq!(payload.payload, b"original-query".to_vec().into());
    assert!(ext_unknown.is_empty());
    assert_eq!(retained_query_pointer, Some(original_query_pointer));
    assert_eq!(retained_query_owner, Some(original_query_pointer));
    assert_eq!(original_timeout_cancellation, Some(()));
    assert_eq!(original_request.id, 37);
    assert_eq!(second_request.id, 41);
    assert_eq!(third_request.id, 43);
    assert_eq!(original_completion.len(), 1);
    assert_eq!(original_completion[0].rid, 37);
    for (replies, expected) in [(&second_responses, 41), (&third_responses, 43)] {
        for reply in replies {
            assert_eq!(reply.rid, expected);
            let ResponseBody::Reply(Reply {
                payload: PushBody::Put(payload),
                ..
            }) = &reply.payload
            else {
                panic!("every forwarded query keeps its native reply body");
            };
            assert_ne!(payload.payload, b"original-query".to_vec().into());
        }
    }
    assert_eq!(second_completion.len(), 1);
    assert_eq!(second_completion[0].rid, 41);
    assert_eq!(third_completion.len(), 1);
    assert_eq!(third_completion[0].rid, 43);
    assert_eq!(original_replies.len(), routed_requests.len());
    assert_eq!(original_finals.len(), routed_requests.len());
    assert_eq!(remaining_queries, 0);
    assert_eq!(original_query.strong_count(), 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn routed_queries_issue_the_last_identifier_once_and_preserve_target_selection() {
    for target in [
        QueryTarget::All,
        QueryTarget::AllComplete,
        QueryTarget::BestMatching,
    ] {
        let router = HarnessBuilder::new()
            .mode(WhatAmI::Router)
            .subregions([Region::Local])
            .build();
        let caller = router.new_session();
        let nearest_queryable = router.new_session();
        let farther_queryable = router.new_session();
        let expression = "query-custody/last-identifier";
        for (queryable, distance) in [(&nearest_queryable, 0), (&farther_queryable, 100)] {
            queryable.declare_queryable(
                None,
                17,
                expression,
                QueryableInfoType {
                    complete: true,
                    distance,
                },
            );
        }
        {
            let queries = router.gateway.tables.queries_lock.write().unwrap();
            let mut face = nearest_queryable.face.state.clone();
            get_mut_unchecked(&mut face).next_qid = u32::MAX - 1;
            drop(queries);
        }
        let mut requests = [61, 63, 65].map(|id| Request {
            id,
            wire_expr: WireExpr::empty().with_suffix(expression).to_owned(),
            ext_qos: network_ext::QoSType::DEFAULT,
            ext_tstamp: None,
            ext_nodeid: network_ext::NodeIdType::DEFAULT,
            ext_target: target,
            ext_budget: None,
            ext_timeout: Some(Duration::from_secs(600)),
            ext_ts_stack: None,
            payload: RequestBody::Query(Query {
                parameters: id.to_string(),
                ..Query::default()
            }),
        });
        for request in &mut requests {
            caller.face.send_request(request);
        }
        let nearest_requests = nearest_queryable.recorder().requests();
        let farther_requests = farther_queryable.recorder().requests();
        let last_identifier = nearest_requests
            .first()
            .expect("the nearest complete queryable must receive the last usable query")
            .id;
        let (last_query, last_timeout, pending_before_replies, sequence_after_requests) = {
            let queries = router.gateway.tables.queries_lock.read().unwrap();
            let face = &nearest_queryable.face.state;
            let (query, cancellation) = face
                .pending_queries
                .get(&last_identifier)
                .expect("the last usable query retains its original registration");
            let owned = (
                Arc::downgrade(query),
                cancellation.clone(),
                face.pending_queries.len(),
                face.next_qid,
            );
            drop(queries);
            owned
        };
        let mut finals = Vec::new();
        for (queryable, forwarded, tag) in [
            (&nearest_queryable, &nearest_requests, "last-query"),
            (&farther_queryable, &farther_requests, "other-query"),
        ] {
            for request in forwarded {
                let RequestBody::Query(query) = &request.payload;
                let tag = format!("{tag}:{}", query.parameters);
                let mut reply = Response {
                    rid: request.id,
                    wire_expr: WireExpr::empty().with_suffix(expression).to_owned(),
                    payload: ResponseBody::Reply(Reply {
                        consolidation: ConsolidationMode::None,
                        ext_unknown: Vec::new(),
                        payload: PushBody::Put(Put {
                            payload: tag.into_bytes().into(),
                            ..Put::default()
                        }),
                    }),
                    ext_qos: network_ext::QoSType::DEFAULT,
                    ext_tstamp: None,
                    ext_respid: None,
                    ext_ts_stack: None,
                };
                queryable.face.send_response(&mut reply);
                let mut final_reply = ResponseFinal {
                    rid: request.id,
                    ext_qos: network_ext::QoSType::DEFAULT,
                    ext_tstamp: None,
                };
                queryable.face.send_response_final(&mut final_reply);
                finals.push(final_reply);
            }
        }
        let before_replay = {
            let queries = router.gateway.tables.queries_lock.read().unwrap();
            let retained = (
                last_query.strong_count(),
                nearest_queryable
                    .face
                    .state
                    .pending_queries
                    .get(&last_identifier)
                    .map(|(query, _)| Arc::as_ptr(query)),
                last_timeout.cancelled().now_or_never(),
            );
            drop(queries);
            retained
        };
        let mut duplicate_final = ResponseFinal {
            rid: last_identifier,
            ext_qos: network_ext::QoSType::DEFAULT,
            ext_tstamp: None,
        };
        nearest_queryable
            .face
            .send_response_final(&mut duplicate_final);
        let original_cancellation = last_timeout.cancelled().now_or_never();
        let responses = caller.recorder().responses();
        let completion = caller.recorder().response_finals();
        let mut closed_faces = Vec::new();
        let mut joined_tasks = Vec::new();
        let mut closed_runtime = None;
        let cleanup = tokio::time::timeout(Duration::from_secs(60), async {
            for face in [&nearest_queryable, &farther_queryable, &caller] {
                let () = face.face.send_close();
                closed_faces.push(());
                let joined = face.face.state.task_controller.terminate_all_async().await;
                joined_tasks.push(joined);
            }
            closed_runtime = Some(CloseBuilder::new(router._runtime.as_ref().unwrap()).await);
        })
        .await;
        assert!(
            cleanup.is_ok(),
            "native router cleanup must precede the oracle"
        );
        assert_eq!(closed_faces, [(), (), ()]);
        assert_eq!(joined_tasks, [(), (), ()]);
        assert!(matches!(closed_runtime, Some(Ok(()))));
        assert_eq!(
            nearest_requests.len(),
            1,
            "exhausted face issued another query"
        );
        assert_eq!(last_identifier, u32::MAX);
        assert_eq!(pending_before_replies, 1);
        assert_eq!(sequence_after_requests, u32::MAX);
        assert_eq!(original_cancellation, Some(()));
        assert_eq!(last_query.strong_count(), 0);
        let caller_ids = requests.each_ref().map(|request| request.id);
        assert_eq!(caller_ids, [61, 63, 65]);
        assert_eq!(before_replay, (0, None, Some(())));
        let expected_farther = match target {
            QueryTarget::All | QueryTarget::AllComplete => 3,
            QueryTarget::BestMatching => 0,
        };
        assert_eq!(farther_requests.len(), expected_farther);
        assert_eq!(responses.len(), 1 + expected_farther);
        let mut delivered = Vec::new();
        for reply in responses {
            let ResponseBody::Reply(Reply {
                consolidation: ConsolidationMode::None,
                ext_unknown,
                payload: PushBody::Put(payload),
            }) = reply.payload
            else {
                panic!("caller keeps the complete native reply");
            };
            assert!(ext_unknown.is_empty());
            delivered.push((reply.rid, payload.payload));
        }
        delivered.sort_by_key(|(caller, _)| *caller);
        let mut expected = vec![(61, b"last-query:61".to_vec().into())];
        match target {
            QueryTarget::All | QueryTarget::AllComplete => {
                expected.extend([
                    (61, b"other-query:61".to_vec().into()),
                    (63, b"other-query:63".to_vec().into()),
                    (65, b"other-query:65".to_vec().into()),
                ]);
            }
            QueryTarget::BestMatching => {}
        }
        assert_eq!(delivered, expected);
        let mut completed_ids: Vec<_> = completion.iter().map(|reply| reply.rid).collect();
        completed_ids.sort_unstable();
        assert_eq!(completed_ids, [61, 63, 65]);
        assert_eq!(finals.len(), 1 + expected_farther);
        assert_eq!(duplicate_final.rid, last_identifier);
    }
}
