//
// Copyright (c) 2023 ZettaScale Technology
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
use std::{
    env,
    io::{self, BufRead, Write},
    path::Path,
    process::{ChildStdin, Command, Stdio},
    sync::Arc,
    time::Duration,
};

use serde::{Deserialize, Serialize};
use serde_json::json;
use tokio::time::{sleep, timeout};
use zenoh_config::Config;
use zenoh_core::zlock;
use zenoh_protocol::{
    core::{key_expr::keyexpr, ExprId, Region, Reliability, WhatAmI, WireExpr, EMPTY_EXPR_ID},
    network::{
        declare::{self, queryable::ext::QueryableInfoType},
        ext::{self, NodeIdType},
        request::ext::QueryTarget,
        Declare, DeclareBody, DeclareKeyExpr, Mapping, NetworkBody, NetworkMessage, Push, Request,
        Response, UndeclareKeyExpr,
    },
    zenoh::{PushBody, Put, RequestBody, ResponseBody},
};
use zenoh_result::{bail, ZResult};
use zenoh_transport::unicast::TransportUnicast;

use crate::{
    key_expr::KeyExpr,
    net::{
        primitives::{DummyPrimitives, EPrimitives, Primitives},
        routing::{
            dispatcher::{
                face::{Face, FaceState},
                pubsub::SubscriberInfo,
                queries::route_send_response,
                tables::TablesData,
            },
            gateway::*,
            RoutingContext,
        },
    },
    Config as SessionConfig, Session,
};

const DECLARATION_NAMESPACE: &str = "research/pressure/outside";
const DECLARATION_WAIT: Duration = Duration::from_secs(5);

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
enum DeclarationCommand {
    Extend,
    Release,
    Exit,
}

#[derive(Debug)]
enum DeclarationScheduling {
    Queued,
    NotQueued,
}

#[derive(Debug, Default, PartialEq)]
struct DeclarationInventory {
    remote_mappings: usize,
    mapped_expression_bytes: usize,
    namespace_nodes: usize,
    stored_expression_bytes: usize,
    string_capacity_bytes: usize,
}

fn declaration_config(
    mode: WhatAmI,
    listen: Vec<&str>,
    connect: Vec<&str>,
    certificates: &Path,
) -> ZResult<SessionConfig> {
    SessionConfig::from_json5(
        &json!({
            "mode": mode,
            "listen": { "endpoints": listen },
            "connect": { "endpoints": connect, "timeout_ms": 2000, "exit_on_failure": true },
            "scouting": {
                "multicast": { "enabled": false, "listen": false },
                "gossip": { "enabled": false }
            },
            "transport": { "link": { "protocols": ["tls"], "tls": {
                "root_ca_certificate": certificates.join("root-ca.pem"),
                "listen_certificate": certificates.join("localhost-certificate.pem"),
                "listen_private_key": certificates.join("localhost-private-key.pem"),
                "connect_certificate": certificates.join("localhost-certificate.pem"),
                "connect_private_key": certificates.join("localhost-private-key.pem"),
                "enable_mtls": true, "verify_name_on_connect": true, "use_public_pki": false
            } } }
        })
        .to_string(),
    )
}

fn declaration_inventory(session: &Session) -> DeclarationInventory {
    let gateway = session.runtime().static_runtime().unwrap().router();
    let tables = gateway.tables.tables.read().unwrap();
    let mut inventory = DeclarationInventory::default();
    for face in tables.data.faces.values() {
        for resource in face.remote_mappings.values() {
            if resource
                .expr
                .strip_prefix(DECLARATION_NAMESPACE)
                .is_some_and(|suffix| suffix.is_empty() || suffix.starts_with('/'))
            {
                inventory.remote_mappings += 1;
                inventory.mapped_expression_bytes += resource.expr.len();
            }
        }
    }
    // Borrowing under this lock leaves native strong-count cleanup unchanged.
    let mut resources = vec![tables.data._get_root().as_ref()];
    while let Some(resource) = resources.pop() {
        if resource
            .expr
            .strip_prefix(DECLARATION_NAMESPACE)
            .is_some_and(|suffix| suffix.is_empty() || suffix.starts_with('/'))
        {
            inventory.namespace_nodes += 1;
            inventory.stored_expression_bytes += resource.expr.len();
            inventory.string_capacity_bytes += resource.expr.capacity();
        }
        resources.extend(resource.children.iter().map(|child| child.as_ref()));
    }
    inventory
}

async fn await_declaration_inventory(
    session: &Session,
    expected_mappings: usize,
) -> ZResult<DeclarationInventory> {
    let observed = timeout(DECLARATION_WAIT, async {
        loop {
            let inventory = declaration_inventory(session);
            if inventory.remote_mappings == expected_mappings {
                return inventory;
            }
            sleep(Duration::from_millis(10)).await;
        }
    })
    .await;
    match observed {
        Ok(inventory) => Ok(inventory),
        Err(expiry) => bail!(
            "expected {expected_mappings} remote mappings; observed {:?}: {expiry}",
            declaration_inventory(session)
        ),
    }
}

fn request_declarations(stdin: &mut ChildStdin, command: DeclarationCommand) -> ZResult<()> {
    serde_json::to_writer(&mut *stdin, &command)?;
    stdin.write_all(b"\n")?;
    stdin.flush()?;
    Ok(())
}

fn schedule_declaration(
    transport: &TransportUnicast,
    body: DeclareBody,
) -> ZResult<DeclarationScheduling> {
    let mut message: NetworkMessage = NetworkBody::Declare(Declare {
        interest_id: None,
        ext_qos: declare::ext::QoSType::DECLARE,
        ext_tstamp: None,
        ext_nodeid: declare::ext::NodeIdType::DEFAULT,
        body,
    })
    .into();
    transport
        .schedule(message.as_mut())
        .map(|queued| match queued {
            true => DeclarationScheduling::Queued,
            false => DeclarationScheduling::NotQueued,
        })
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "producer subprocess selected by the owning declaration-pressure test"]
async fn declaration_producer() {
    let config =
        SessionConfig::from_json5(&env::var("BOMBAY_DECLARATION_CONFIG").unwrap()).unwrap();
    let session = crate::open(config).await.unwrap();
    let transports = session.runtime().static_runtime().unwrap().manager();
    let mut transport = None;
    let mut declarations = Vec::new();
    let mut declaration_receipts = Vec::new();
    let mut undeclaration_receipts = Vec::new();
    let work: ZResult<()> = async {
        transport = Some(
            timeout(DECLARATION_WAIT, async {
                loop {
                    if let Some(transport) = transports.get_transports_unicast().await.pop() {
                        return transport;
                    }
                    sleep(Duration::from_millis(10)).await;
                }
            })
            .await?,
        );
        let transport = transport.as_ref().unwrap();
        for line in io::stdin().lock().lines() {
            match serde_json::from_str::<DeclarationCommand>(&line?)? {
                DeclarationCommand::Extend => {
                    let next = declarations.len();
                    for index in next..next + 8 {
                        let id = ExprId::try_from(40000 + index)?;
                        declaration_receipts.push(schedule_declaration(
                            transport,
                            DeclareBody::DeclareKeyExpr(DeclareKeyExpr {
                                id,
                                wire_expr: format!("{DECLARATION_NAMESPACE}/{index:08}").into(),
                            }),
                        ));
                        declarations.push(id);
                    }
                }
                DeclarationCommand::Release => {
                    for id in declarations.drain(..) {
                        undeclaration_receipts.push(schedule_declaration(
                            transport,
                            DeclareBody::UndeclareKeyExpr(UndeclareKeyExpr { id }),
                        ));
                    }
                }
                DeclarationCommand::Exit => break,
            }
        }
        Ok(())
    }
    .await;
    // Independent cleanup still runs after control or declaration failure.
    if let Some(transport) = transport {
        for id in declarations {
            undeclaration_receipts.push(schedule_declaration(
                &transport,
                DeclareBody::UndeclareKeyExpr(UndeclareKeyExpr { id }),
            ));
        }
    }
    let session_close = session.close().await;
    assert!(work.is_ok() && declaration_receipts.iter().all(|receipt| matches!(receipt, Ok(DeclarationScheduling::Queued)))
        && undeclaration_receipts.iter().all(|receipt| matches!(receipt, Ok(DeclarationScheduling::Queued))) && session_close.is_ok(),
        "producer work: {work:?}; native declarations: {declaration_receipts:?}; native undeclarations: {undeclaration_receipts:?}; session close: {session_close:?}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires BOMBAY_DECLARATION_CERTIFICATES containing the public TLS fixture"]
async fn declarations_outside_acl_retain_resources() {
    let certificates = env::var("BOMBAY_DECLARATION_CERTIFICATES").unwrap();
    let certificates = Path::new(&certificates);
    for owner_mode in [WhatAmI::Peer, WhatAmI::Router] {
        let producer_mode = match owner_mode {
            WhatAmI::Peer => WhatAmI::Peer,
            WhatAmI::Router => WhatAmI::Client,
            WhatAmI::Client => unreachable!("the owner must receive declarations"),
        };
        let mut config =
            declaration_config(owner_mode, vec!["tls/127.0.0.1:0"], vec![], certificates).unwrap();
        config
            .insert_json5(
                "access_control",
                &json!({
                    "enabled": true, "default_permission": "deny",
                    "rules": [{ "id": "health", "permission": "allow",
                        "flows": ["ingress", "egress"], "messages": ["put", "declare_subscriber"],
                        "key_exprs": ["research/pressure/health"] }],
                    "subjects": [{ "id": "all" }],
                    "policies": [{ "rules": ["health"], "subjects": ["all"] }]
                })
                .to_string(),
            )
            .unwrap();
        let session = crate::open(config).await.unwrap();
        let mut inventories = Vec::new();
        let mut producer = None;
        let mut producer_stdin = None;
        let work: ZResult<()> = async {
            let locators = session.runtime().static_runtime().unwrap().get_locators();
            let port = locators
                .first()
                .ok_or("no TLS listener locator")?
                .address()
                .as_str()
                .rsplit(':')
                .next()
                .ok_or("no TLS listener port")?;
            let address = format!("tls/localhost:{port}");
            let producer_config =
                declaration_config(producer_mode, vec![], vec![&address], certificates)?;
            let mut child = Command::new(env::current_exe()?)
                .args([
                    "--ignored",
                    "--exact",
                    "net::tests::tables::declaration_producer",
                    "--nocapture",
                ])
                .env(
                    "BOMBAY_DECLARATION_CONFIG",
                    serde_json::to_string(&producer_config)?,
                )
                .stdin(Stdio::piped())
                .spawn()?;
            producer_stdin = child.stdin.take();
            producer = Some(child);
            let stdin = producer_stdin.as_mut().ok_or("missing producer stdin")?;
            for expected in [8, 16] {
                request_declarations(stdin, DeclarationCommand::Extend)?;
                inventories.push(await_declaration_inventory(&session, expected).await?);
            }
            request_declarations(stdin, DeclarationCommand::Release)?;
            inventories.push(await_declaration_inventory(&session, 0).await?);
            Ok(())
        }
        .await;
        let exit_request = producer_stdin
            .as_mut()
            .map(|stdin| request_declarations(stdin, DeclarationCommand::Exit));
        drop(producer_stdin);
        let producer_exit = producer.as_mut().map(|child| child.wait());
        let final_inventory = declaration_inventory(&session);
        let session_close = session.close().await;
        // This proves retained growth and native restoration, not enforced bounds,
        // whole-heap memory, or retirement of every transport callback.
        println!("{owner_mode:?}: inventories={inventories:?}; final={final_inventory:?}");
        assert!(work.is_ok() && matches!(exit_request, Some(Ok(())))
            && matches!(producer_exit, Some(Ok(status)) if status.success()) && session_close.is_ok(),
            "work: {work:?}; exit request: {exit_request:?}; producer exit: {producer_exit:?}; session close: {session_close:?}");
        for (inventory, expected) in inventories[..2].iter().zip([8, 16]) {
            assert_eq!(inventory.remote_mappings, expected);
            assert_eq!(inventory.mapped_expression_bytes, expected * 34);
            assert_eq!(inventory.namespace_nodes, expected + 1);
            assert_eq!(inventory.stored_expression_bytes, 25 + expected * 34);
            assert!(inventory.string_capacity_bytes >= inventory.stored_expression_bytes);
        }
        assert_eq!(inventories[2], DeclarationInventory::default());
        assert_eq!(final_inventory, DeclarationInventory::default());
    }
}

fn new_router() -> Gateway {
    let mut config = Config::default().expanded();
    config.set_mode(Some(WhatAmI::Client)).unwrap();
    GatewayBuilder::new(&config)
        .subregions(vec![Region::Local])
        .build()
        .unwrap()
}

#[test]
fn base_test() {
    let router = new_router();
    let tables = router.tables.clone();

    let primitives = Arc::new(DummyPrimitives {});
    let face = router.new_session(primitives);
    register_expr(&tables, &mut face.state.clone(), 1, &"one/two/three".into());

    register_expr(
        &tables,
        &mut face.state.clone(),
        2,
        &"one/deux/trois".into(),
    );

    let sub_info = SubscriberInfo;

    face.declare_subscriber(
        0,
        &WireExpr::from(1).with_suffix("four/five"),
        &sub_info,
        NodeId::default(),
        &mut |p, m| {
            m.with_mut(|m| {
                p.send_declare(m);
            })
        },
    );

    TablesData::print(&zread!(tables.tables).data);
}

#[test]
fn match_test() {
    let key_exprs = [
        "**",
        "a",
        "a/b",
        "*",
        "a/*",
        "a/b$*",
        "abc",
        "xx",
        "ab$*",
        "abcd",
        "ab$*d",
        "ab",
        "ab/*",
        "a/*/c/*/e",
        "a/b/c/d/e",
        "a/$*b/c/$*d/e",
        "a/xb/c/xd/e",
        "a/c/e",
        "a/b/c/d/x/e",
        "ab$*cd",
        "abxxcxxd",
        "abxxcxxcd",
        "abxxcxxcdx",
        "a/b/c",
        "ab/**",
        "**/xyz",
        "a/b/xyz/d/e/f/xyz",
        "**/xyz$*xyz",
        "a/b/xyz/d/e/f/xyz",
        "a/**/c/**/e",
        "a/b/b/b/c/d/d/d/e",
        "a/**/c/*/e/*",
        "a/b/b/b/c/d/d/c/d/e/f",
        "a/**/c/*/e/*",
        "x/abc",
        "x/*",
        "x/abc$*",
        "x/$*abc",
        "x/a$*",
        "x/a$*de",
        "x/abc$*de",
        "x/a$*d$*e",
        "x/a$*e",
        "x/a$*c$*e",
        "x/ade",
        "x/c$*",
        "x/$*d",
        "x/$*e",
        "@a",
        "**/@a",
        "@a/b",
    ]
    .map(|s| keyexpr::new(s).unwrap());

    let router = new_router();
    let tables = router.tables.clone();

    let primitives = Arc::new(DummyPrimitives {});
    let face = Arc::downgrade(&router.new_session(primitives).state);
    for (i, key_expr) in key_exprs.iter().enumerate() {
        register_expr(
            &tables,
            &mut face.upgrade().unwrap(),
            i.try_into().unwrap(),
            &(*key_expr).into(),
        );
    }

    for key_expr1 in key_exprs.iter() {
        let res_matches = Resource::get_matches(&zread!(tables.tables).data, key_expr1);
        for key_expr2 in key_exprs.iter() {
            if res_matches
                .iter()
                .any(|m| m.upgrade().unwrap().expr() == key_expr2.as_str())
            {
                assert!(dbg!(dbg!(key_expr1).intersects(dbg!(key_expr2))));
            } else {
                assert!(!dbg!(dbg!(key_expr1).intersects(dbg!(key_expr2))));
            }
        }
    }
}

#[test]
fn multisub_test() {
    let router = new_router();
    let tables = router.tables.clone();

    let primitives = Arc::new(DummyPrimitives {});
    let face0 = &router.new_session(primitives);

    // --------------
    let sub_info = SubscriberInfo;
    face0.declare_subscriber(
        0,
        &"sub".into(),
        &sub_info,
        NodeId::default(),
        &mut |p, m| {
            m.with_mut(|m| {
                p.send_declare(m);
            })
        },
    );
    let optres = Resource::get_resource(zread!(tables.tables).data._get_root(), "sub")
        .map(|res| Arc::downgrade(&res));
    assert!(optres.is_some());
    let res = optres.unwrap();
    assert!(res.upgrade().is_some());

    face0.declare_subscriber(
        1,
        &"sub".into(),
        &sub_info,
        NodeId::default(),
        &mut |p, m| {
            m.with_mut(|m| {
                p.send_declare(m);
            })
        },
    );
    assert!(res.upgrade().is_some());

    face0.undeclare_subscriber(0, &WireExpr::empty(), NodeId::default(), &mut |p, m| {
        m.with_mut(|m| {
            p.send_declare(m);
        })
    });
    assert!(res.upgrade().is_some());

    face0.undeclare_subscriber(1, &WireExpr::empty(), NodeId::default(), &mut |p, m| {
        m.with_mut(|m| {
            p.send_declare(m);
        })
    });
    assert!(res.upgrade().is_none());

    face0.send_close();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn clean_test() {
    let router = new_router();
    let tables = router.tables.clone();

    let primitives = Arc::new(DummyPrimitives {});
    let face0 = &router.new_session(primitives);

    // --------------
    register_expr(&tables, &mut face0.state.clone(), 1, &"todrop1".into());
    let optres1 = Resource::get_resource(zread!(tables.tables).data._get_root(), "todrop1")
        .map(|res| Arc::downgrade(&res));
    assert!(optres1.is_some());
    let res1 = optres1.unwrap();
    assert!(res1.upgrade().is_some());

    register_expr(
        &tables,
        &mut face0.state.clone(),
        2,
        &"todrop1/todrop11".into(),
    );
    let optres2 =
        Resource::get_resource(zread!(tables.tables).data._get_root(), "todrop1/todrop11")
            .map(|res| Arc::downgrade(&res));
    assert!(optres2.is_some());
    let res2 = optres2.unwrap();
    assert!(res2.upgrade().is_some());

    register_expr(&tables, &mut face0.state.clone(), 3, &"**".into());
    let optres3 = Resource::get_resource(zread!(tables.tables).data._get_root(), "**")
        .map(|res| Arc::downgrade(&res));
    assert!(optres3.is_some());
    let res3 = optres3.unwrap();
    assert!(res3.upgrade().is_some());

    unregister_expr(&tables, &mut face0.state.clone(), 1);
    assert!(res1.upgrade().is_some());
    assert!(res2.upgrade().is_some());
    assert!(res3.upgrade().is_some());

    unregister_expr(&tables, &mut face0.state.clone(), 2);
    assert!(res1.upgrade().is_none());
    assert!(res2.upgrade().is_none());
    assert!(res3.upgrade().is_some());

    unregister_expr(&tables, &mut face0.state.clone(), 3);
    assert!(res1.upgrade().is_none());
    assert!(res2.upgrade().is_none());
    assert!(res3.upgrade().is_none());

    // --------------
    register_expr(&tables, &mut face0.state.clone(), 1, &"todrop1".into());
    let optres1 = Resource::get_resource(zread!(tables.tables).data._get_root(), "todrop1")
        .map(|res| Arc::downgrade(&res));
    assert!(optres1.is_some());
    let res1 = optres1.unwrap();
    assert!(res1.upgrade().is_some());

    let sub_info = SubscriberInfo;

    face0.declare_subscriber(
        0,
        &"todrop1/todrop11".into(),
        &sub_info,
        NodeId::default(),
        &mut |p, m| {
            m.with_mut(|m| {
                p.send_declare(m);
            })
        },
    );
    let optres2 =
        Resource::get_resource(zread!(tables.tables).data._get_root(), "todrop1/todrop11")
            .map(|res| Arc::downgrade(&res));
    assert!(optres2.is_some());
    let res2 = optres2.unwrap();
    assert!(res2.upgrade().is_some());

    face0.declare_subscriber(
        1,
        &WireExpr::from(1).with_suffix("/todrop12"),
        &sub_info,
        NodeId::default(),
        &mut |p, m| {
            m.with_mut(|m| {
                p.send_declare(m);
            })
        },
    );
    let optres3 =
        Resource::get_resource(zread!(tables.tables).data._get_root(), "todrop1/todrop12")
            .map(|res| Arc::downgrade(&res));
    assert!(optres3.is_some());
    let res3 = optres3.unwrap();
    println!("COUNT: {}", res3.strong_count());
    assert!(res3.upgrade().is_some());

    face0.undeclare_subscriber(1, &WireExpr::empty(), NodeId::default(), &mut |p, m| {
        m.with_mut(|m| {
            p.send_declare(m);
        })
    });

    println!("COUNT2: {}", res3.strong_count());

    assert!(res1.upgrade().is_some());
    assert!(res2.upgrade().is_some());
    assert!(res3.upgrade().is_none());

    face0.undeclare_subscriber(0, &WireExpr::empty(), NodeId::default(), &mut |p, m| {
        m.with_mut(|m| {
            p.send_declare(m);
        })
    });
    assert!(res1.upgrade().is_some());
    assert!(res2.upgrade().is_none());
    assert!(res3.upgrade().is_none());

    unregister_expr(&tables, &mut face0.state.clone(), 1);
    assert!(res1.upgrade().is_none());
    assert!(res2.upgrade().is_none());
    assert!(res3.upgrade().is_none());

    // --------------
    register_expr(&tables, &mut face0.state.clone(), 2, &"todrop3".into());
    face0.declare_subscriber(
        2,
        &"todrop3".into(),
        &sub_info,
        NodeId::default(),
        &mut |p, m| {
            m.with_mut(|m| {
                p.send_declare(m);
            })
        },
    );
    let optres1 = Resource::get_resource(zread!(tables.tables).data._get_root(), "todrop3")
        .map(|res| Arc::downgrade(&res));
    assert!(optres1.is_some());
    let res1 = optres1.unwrap();
    assert!(res1.upgrade().is_some());

    face0.undeclare_subscriber(2, &WireExpr::empty(), NodeId::default(), &mut |p, m| {
        m.with_mut(|m| {
            p.send_declare(m);
        })
    });
    assert!(res1.upgrade().is_some());

    unregister_expr(&tables, &mut face0.state.clone(), 2);
    assert!(res1.upgrade().is_none());

    // --------------
    register_expr(&tables, &mut face0.state.clone(), 3, &"todrop4".into());
    register_expr(&tables, &mut face0.state.clone(), 4, &"todrop5".into());
    register_expr(&tables, &mut face0.state.clone(), 5, &"todrop6".into());
    face0.declare_subscriber(
        3,
        &WireExpr::from(4),
        &sub_info,
        NodeId::default(),
        &mut |p, m| {
            m.with_mut(|m| {
                p.send_declare(m);
            })
        },
    );
    face0.declare_subscriber(
        4,
        &"todrop7".into(),
        &sub_info,
        NodeId::default(),
        &mut |p, m| {
            m.with_mut(|m| {
                p.send_declare(m);
            })
        },
    );
    face0.declare_token(
        5,
        &WireExpr::from(4),
        NodeId::default(),
        None,
        &mut |p, m| {
            m.with_mut(|m| {
                p.send_declare(m);
            })
        },
    );
    face0.declare_token(
        4,
        &"todrop8".into(),
        NodeId::default(),
        None,
        &mut |p, m| {
            m.with_mut(|m| {
                p.send_declare(m);
            })
        },
    );

    let optres1 = Resource::get_resource(zread!(tables.tables).data._get_root(), "todrop4")
        .map(|res| Arc::downgrade(&res));
    assert!(optres1.is_some());
    let res1 = optres1.unwrap();
    let optres2 = Resource::get_resource(zread!(tables.tables).data._get_root(), "todrop5")
        .map(|res| Arc::downgrade(&res));
    assert!(optres2.is_some());
    let res2 = optres2.unwrap();
    let optres3 = Resource::get_resource(zread!(tables.tables).data._get_root(), "todrop6")
        .map(|res| Arc::downgrade(&res));
    assert!(optres3.is_some());
    let res3 = optres3.unwrap();
    let optres4 = Resource::get_resource(zread!(tables.tables).data._get_root(), "todrop7")
        .map(|res| Arc::downgrade(&res));
    assert!(optres4.is_some());
    let res4 = optres4.unwrap();
    let optres5 = Resource::get_resource(zread!(tables.tables).data._get_root(), "todrop8")
        .map(|res| Arc::downgrade(&res));
    assert!(optres5.is_some());
    let res5 = optres5.unwrap();

    assert!(res1.upgrade().is_some());
    assert!(res2.upgrade().is_some());
    assert!(res3.upgrade().is_some());
    assert!(res4.upgrade().is_some());
    assert!(res5.upgrade().is_some());

    face0.send_close();
    assert!(res1.upgrade().is_none());
    assert!(res2.upgrade().is_none());
    assert!(res3.upgrade().is_none());
    assert!(res4.upgrade().is_none());
    assert!(res5.upgrade().is_none());
}

pub struct ClientPrimitives {
    data: std::sync::Mutex<Option<WireExpr<'static>>>,
    mapping: std::sync::Mutex<std::collections::HashMap<ExprId, String>>,
}

impl ClientPrimitives {
    pub fn new() -> ClientPrimitives {
        ClientPrimitives {
            data: std::sync::Mutex::new(None),
            mapping: std::sync::Mutex::new(std::collections::HashMap::new()),
        }
    }

    pub fn clear_data(&self) {
        *self.data.lock().unwrap() = None;
    }
}

impl Default for ClientPrimitives {
    fn default() -> Self {
        Self::new()
    }
}

impl ClientPrimitives {
    fn get_name(&self, key_expr: &WireExpr) -> String {
        let mapping = self.mapping.lock().unwrap();
        let (scope, suffix) = key_expr.as_id_and_suffix();
        if scope == EMPTY_EXPR_ID {
            suffix.to_string()
        } else if suffix.is_empty() {
            mapping.get(&scope).unwrap().clone()
        } else {
            format!("{}{}", mapping.get(&scope).unwrap(), suffix)
        }
    }

    fn get_last_name(&self) -> Option<String> {
        self.data
            .lock()
            .unwrap()
            .as_ref()
            .map(|data| self.get_name(data))
    }

    #[allow(dead_code)]
    fn get_last_key(&self) -> Option<WireExpr<'_>> {
        self.data.lock().unwrap().as_ref().cloned()
    }
}

impl Primitives for ClientPrimitives {
    fn send_interest(&self, _msg: &mut zenoh_protocol::network::Interest) {}

    fn send_declare(&self, msg: &mut zenoh_protocol::network::Declare) {
        match &msg.body {
            DeclareBody::DeclareKeyExpr(d) => {
                let name = self.get_name(&d.wire_expr);
                zlock!(self.mapping).insert(d.id, name);
            }
            DeclareBody::UndeclareKeyExpr(u) => {
                zlock!(self.mapping).remove(&u.id);
            }
            _ => (),
        }
    }

    fn send_push_consume(
        &self,
        msg: &mut zenoh_protocol::network::Push,
        _reliability: Reliability,
        _consume: bool,
    ) {
        *zlock!(self.data) = Some(msg.wire_expr.to_owned());
    }

    fn send_request(&self, msg: &mut zenoh_protocol::network::Request) {
        *zlock!(self.data) = Some(msg.wire_expr.to_owned())
    }

    fn send_response(&self, msg: &mut zenoh_protocol::network::Response) {
        *zlock!(self.data) = Some(msg.wire_expr.to_owned())
    }

    fn send_response_final(&self, _msg: &mut zenoh_protocol::network::ResponseFinal) {}

    fn send_close(&self) {}

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

impl EPrimitives for ClientPrimitives {
    fn send_interest(&self, _ctx: RoutingContext<&mut zenoh_protocol::network::Interest>) -> bool {
        false
    }

    fn send_declare(&self, ctx: RoutingContext<&mut zenoh_protocol::network::Declare>) -> bool {
        match &ctx.msg.body {
            DeclareBody::DeclareKeyExpr(d) => {
                let name = self.get_name(&d.wire_expr);
                zlock!(self.mapping).insert(d.id, name);
            }
            DeclareBody::UndeclareKeyExpr(u) => {
                zlock!(self.mapping).remove(&u.id);
            }
            _ => (),
        }
        false
    }

    fn send_push(
        &self,
        msg: &mut zenoh_protocol::network::Push,
        _reliability: Reliability,
    ) -> bool {
        *zlock!(self.data) = Some(msg.wire_expr.to_owned());
        false
    }

    fn send_request(&self, msg: &mut zenoh_protocol::network::Request) -> bool {
        *zlock!(self.data) = Some(msg.wire_expr.to_owned());
        false
    }

    fn send_response(&self, msg: &mut zenoh_protocol::network::Response) -> bool {
        *zlock!(self.data) = Some(msg.wire_expr.to_owned());
        false
    }

    fn send_response_final(&self, _msg: &mut zenoh_protocol::network::ResponseFinal) -> bool {
        false
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

#[test]
fn test_response_wireexpr() {
    let router = new_router();
    let tables = router.tables.clone();
    let primitives0 = Arc::new(ClientPrimitives::new());
    let face0 = &router.new_session(primitives0.clone());

    let primitives1 = Arc::new(ClientPrimitives::new());
    let face1 = &router.new_session(primitives1.clone());

    let qinfo = QueryableInfoType {
        complete: true,
        distance: 1,
    };
    register_expr(
        &tables,
        &mut face0.state.clone(),
        11,
        &"test/queryable/reply".into(),
    );

    Primitives::send_declare(
        primitives0.as_ref(),
        &mut Declare {
            interest_id: None,
            ext_qos: ext::QoSType::DECLARE,
            ext_tstamp: None,
            ext_nodeid: ext::NodeIdType::DEFAULT,
            body: DeclareBody::DeclareKeyExpr(DeclareKeyExpr {
                id: 11,
                wire_expr: "test/queryable/reply".into(),
            }),
        },
    );

    register_expr(
        &tables,
        &mut face1.state.clone(),
        12,
        &"test/queryable".into(),
    );

    Primitives::send_declare(
        primitives1.as_ref(),
        &mut Declare {
            interest_id: None,
            ext_qos: ext::QoSType::DECLARE,
            ext_tstamp: None,
            ext_nodeid: ext::NodeIdType::DEFAULT,
            body: DeclareBody::DeclareKeyExpr(DeclareKeyExpr {
                id: 12,
                wire_expr: "test/queryable".into(),
            }),
        },
    );

    face0.declare_queryable(
        0,
        &WireExpr::from(11).with_suffix("/**"),
        &qinfo,
        NodeId::default(),
        &mut |p, m| {
            m.with_mut(|m| {
                p.send_declare(m);
            })
        },
    );

    face1.route_query(&mut Request {
        id: 1,
        wire_expr: "test/queryable/reply/*".into(),
        payload: RequestBody::Query(zenoh_protocol::zenoh::query::Query::default()),
        ext_qos: zenoh_protocol::network::request::ext::QoSType::default(),
        ext_tstamp: None,
        ext_nodeid: NodeIdType::DEFAULT,
        ext_target: QueryTarget::All,
        ext_budget: None,
        ext_timeout: None,
        ext_ts_stack: None,
    });

    route_send_response(
        &tables,
        &mut face0.state.clone(),
        &mut Response {
            rid: 1,
            wire_expr: WireExpr {
                scope: 11,
                suffix: "/1".into(),
                mapping: Mapping::Sender,
            },
            payload: ResponseBody::Reply(zenoh_protocol::zenoh::reply::Reply {
                consolidation: zenoh_protocol::zenoh::ConsolidationMode::None,
                ext_unknown: Vec::default(),
                payload: PushBody::Put(zenoh_protocol::zenoh::put::Put::default()),
            }),
            ext_qos: zenoh_protocol::network::response::ext::QoSType::default(),
            ext_tstamp: None,
            ext_respid: None,
            ext_ts_stack: None,
        },
    );
    assert_eq!(
        primitives1.get_last_name().unwrap(),
        "test/queryable/reply/1"
    );
    let we = primitives1.get_last_key().unwrap();
    // TODO: replace asserts with the ones commented below once optimization of reply wireexpr in route_send_response is enabled
    assert_eq!(we.suffix, "test/queryable/reply/1");
    assert_eq!(we.scope, 0);
    /*assert_eq!(we.suffix, "/reply/1");
    assert_eq!(we.scope, 12);
    assert_eq!(we.mapping, Mapping::Receiver);*/

    // unregister receiver mapping and validate that we is still correct
    unregister_expr(&tables, &mut face1.state.clone(), 12);

    Primitives::send_declare(
        primitives1.as_ref(),
        &mut Declare {
            interest_id: None,
            ext_qos: ext::QoSType::DECLARE,
            ext_tstamp: None,
            ext_nodeid: ext::NodeIdType::DEFAULT,
            body: DeclareBody::UndeclareKeyExpr(UndeclareKeyExpr { id: 12 }),
        },
    );

    route_send_response(
        &tables,
        &mut face0.state.clone(),
        &mut Response {
            rid: 1,
            wire_expr: WireExpr {
                scope: 11,
                suffix: "/1".into(),
                mapping: Mapping::Sender,
            },
            payload: ResponseBody::Reply(zenoh_protocol::zenoh::reply::Reply {
                consolidation: zenoh_protocol::zenoh::ConsolidationMode::None,
                ext_unknown: Vec::default(),
                payload: PushBody::Put(zenoh_protocol::zenoh::put::Put::default()),
            }),
            ext_qos: zenoh_protocol::network::response::ext::QoSType::default(),
            ext_tstamp: None,
            ext_respid: None,
            ext_ts_stack: None,
        },
    );
    assert_eq!(
        primitives1.get_last_name().unwrap(),
        "test/queryable/reply/1"
    );
    let we = primitives1.get_last_key().unwrap();
    assert_eq!(we.suffix, "test/queryable/reply/1");
    assert_eq!(we.scope, 0);
}

#[test]
fn client_test() {
    let router = new_router();
    let tables = router.tables.clone();

    let sub_info = SubscriberInfo;

    let primitives0 = Arc::new(ClientPrimitives::new());
    let face0 = router.new_session(primitives0.clone());
    register_expr(&tables, &mut face0.state.clone(), 11, &"test/client".into());
    Primitives::send_declare(
        primitives0.as_ref(),
        &mut Declare {
            interest_id: None,
            ext_qos: ext::QoSType::DECLARE,
            ext_tstamp: None,
            ext_nodeid: ext::NodeIdType::DEFAULT,
            body: DeclareBody::DeclareKeyExpr(DeclareKeyExpr {
                id: 11,
                wire_expr: "test/client".into(),
            }),
        },
    );
    face0.declare_subscriber(
        0,
        &WireExpr::from(11).with_suffix("/**"),
        &sub_info,
        NodeId::default(),
        &mut |p, m| {
            m.with_mut(|m| {
                p.send_declare(m);
            })
        },
    );
    register_expr(
        &tables,
        &mut face0.state.clone(),
        12,
        &WireExpr::from(11).with_suffix("/z1_pub1"),
    );
    Primitives::send_declare(
        primitives0.as_ref(),
        &mut Declare {
            interest_id: None,
            ext_qos: ext::QoSType::DECLARE,
            ext_tstamp: None,
            ext_nodeid: ext::NodeIdType::DEFAULT,
            body: DeclareBody::DeclareKeyExpr(DeclareKeyExpr {
                id: 12,
                wire_expr: WireExpr::from(11).with_suffix("/z1_pub1"),
            }),
        },
    );

    let primitives1 = Arc::new(ClientPrimitives::new());
    let face1 = router.new_session(primitives1.clone());
    register_expr(&tables, &mut face1.state.clone(), 21, &"test/client".into());
    Primitives::send_declare(
        primitives1.as_ref(),
        &mut Declare {
            interest_id: None,
            ext_qos: ext::QoSType::DECLARE,
            ext_tstamp: None,
            ext_nodeid: ext::NodeIdType::DEFAULT,
            body: DeclareBody::DeclareKeyExpr(DeclareKeyExpr {
                id: 21,
                wire_expr: "test/client".into(),
            }),
        },
    );
    face1.declare_subscriber(
        0,
        &WireExpr::from(21).with_suffix("/**"),
        &sub_info,
        NodeId::default(),
        &mut |p, m| {
            m.with_mut(|m| {
                p.send_declare(m);
            })
        },
    );
    register_expr(
        &tables,
        &mut face1.state.clone(),
        22,
        &WireExpr::from(21).with_suffix("/z2_pub1"),
    );
    Primitives::send_declare(
        primitives1.as_ref(),
        &mut Declare {
            interest_id: None,
            ext_qos: ext::QoSType::DECLARE,
            ext_tstamp: None,
            ext_nodeid: ext::NodeIdType::DEFAULT,
            body: DeclareBody::DeclareKeyExpr(DeclareKeyExpr {
                id: 22,
                wire_expr: WireExpr::from(21).with_suffix("/z2_pub1"),
            }),
        },
    );

    let primitives2 = Arc::new(ClientPrimitives::new());
    let face2 = router.new_session(primitives2.clone());
    register_expr(&tables, &mut face2.state.clone(), 31, &"test/client".into());
    Primitives::send_declare(
        primitives2.as_ref(),
        &mut Declare {
            interest_id: None,
            ext_qos: ext::QoSType::DECLARE,
            ext_tstamp: None,
            ext_nodeid: ext::NodeIdType::DEFAULT,
            body: DeclareBody::DeclareKeyExpr(DeclareKeyExpr {
                id: 31,
                wire_expr: "test/client".into(),
            }),
        },
    );
    face2.declare_subscriber(
        0,
        &WireExpr::from(31).with_suffix("/**"),
        &sub_info,
        NodeId::default(),
        &mut |p, m| {
            m.with_mut(|m| {
                p.send_declare(m);
            })
        },
    );

    primitives0.clear_data();
    primitives1.clear_data();
    primitives2.clear_data();

    let route_dummy_data = |face: &Arc<FaceState>, wire_expr| {
        route_data(
            &tables,
            face,
            &mut Push {
                wire_expr,
                ..Put::default().into()
            },
            Reliability::Reliable,
            true,
        );
    };

    route_dummy_data(&face0.state, "test/client/z1_wr1".into());

    // functional check
    assert!(primitives1.get_last_name().is_some());
    assert_eq!(primitives1.get_last_name().unwrap(), "test/client/z1_wr1");
    // mapping strategy check
    // assert_eq!(primitives1.get_last_key().unwrap(), KeyExpr::IdWithSuffix(21, "/z1_wr1".to_string()));

    // functional check
    assert!(primitives2.get_last_name().is_some());
    assert_eq!(primitives2.get_last_name().unwrap(), "test/client/z1_wr1");
    // mapping strategy check
    // assert_eq!(primitives2.get_last_key().unwrap(), KeyExpr::IdWithSuffix(31, "/z1_wr1".to_string()));

    primitives0.clear_data();
    primitives1.clear_data();
    primitives2.clear_data();
    route_dummy_data(&face0.state, WireExpr::from(11).with_suffix("/z1_wr2"));

    // functional check
    assert!(primitives1.get_last_name().is_some());
    assert_eq!(primitives1.get_last_name().unwrap(), "test/client/z1_wr2");
    // mapping strategy check
    // assert_eq!(primitives1.get_last_key().unwrap(), KeyExpr::IdWithSuffix(21, "/z1_wr2".to_string()));

    // functional check
    assert!(primitives2.get_last_name().is_some());
    assert_eq!(primitives2.get_last_name().unwrap(), "test/client/z1_wr2");
    // mapping strategy check
    // assert_eq!(primitives2.get_last_key().unwrap(), KeyExpr::IdWithSuffix(31, "/z1_wr2".to_string()));

    primitives0.clear_data();
    primitives1.clear_data();
    primitives2.clear_data();
    route_dummy_data(&face1.state, "test/client/**".into());

    // functional check
    assert!(primitives0.get_last_name().is_some());
    assert_eq!(primitives0.get_last_name().unwrap(), "test/client/**");
    // mapping strategy check
    // assert_eq!(primitives1.get_last_key().unwrap(), KeyExpr::IdWithSuffix(11, "/**".to_string()));

    // functional check
    assert!(primitives2.get_last_name().is_some());
    assert_eq!(primitives2.get_last_name().unwrap(), "test/client/**");
    // mapping strategy check
    // assert_eq!(primitives2.get_last_key().unwrap(), KeyExpr::IdWithSuffix(31, "/**".to_string()));

    primitives0.clear_data();
    primitives1.clear_data();
    primitives2.clear_data();
    route_dummy_data(&face0.state, 12.into());

    // functional check
    assert!(primitives1.get_last_name().is_some());
    assert_eq!(primitives1.get_last_name().unwrap(), "test/client/z1_pub1");
    // mapping strategy check
    // assert_eq!(primitives1.get_last_key().unwrap(), KeyExpr::IdWithSuffix(21, "/z1_pub1".to_string()));

    // functional check
    assert!(primitives2.get_last_name().is_some());
    assert_eq!(primitives2.get_last_name().unwrap(), "test/client/z1_pub1");
    // mapping strategy check
    // assert_eq!(primitives2.get_last_key().unwrap(), KeyExpr::IdWithSuffix(31, "/z1_pub1".to_string()));

    primitives0.clear_data();
    primitives1.clear_data();
    primitives2.clear_data();
    route_dummy_data(&face1.state, 22.into());

    // functional check
    assert!(primitives0.get_last_name().is_some());
    assert_eq!(primitives0.get_last_name().unwrap(), "test/client/z2_pub1");
    // mapping strategy check
    // assert_eq!(primitives1.get_last_key().unwrap(), KeyExpr::IdWithSuffix(11, "/z2_pub1".to_string()));

    // functional check
    assert!(primitives2.get_last_name().is_some());
    assert_eq!(primitives2.get_last_name().unwrap(), "test/client/z2_pub1");
    // mapping strategy check
    // assert_eq!(primitives2.get_last_key().unwrap(), KeyExpr::IdWithSuffix(31, "/z2_pub1".to_string()));
}

#[test]
fn get_best_key_test() {
    let router = new_router();

    let primitives = Arc::new(DummyPrimitives {});
    let face1 = router.new_session(primitives.clone());
    let face2 = router.new_session(primitives.clone());
    let face3 = router.new_session(primitives);

    let root = zread!(router.tables.tables).data._get_root().clone();
    let register_expr = |face: &Face, id: ExprId, expr: &str| {
        register_expr(&router.tables, &mut face.state.clone(), id, &expr.into());
    };
    let get_best_key = |resource, suffix, face: &Face| {
        Resource::get_resource(&root, resource)
            .unwrap()
            .get_best_key(suffix, face.state.id)
    };

    register_expr(&face1, 1, "a");
    register_expr(&face2, 2, "a/b");
    register_expr(&face2, 3, "a/b/c");
    register_expr(&face3, 4, "a/d");

    macro_rules! assert_wire_expr {
        ($key:expr, {scope: $scope:expr, suffix: $suffix:expr}) => {
            assert_eq!($key.scope, $scope);
            assert_eq!($key.suffix, $suffix);
        };
    }
    assert_wire_expr!(get_best_key("", "a", &face1), { scope: 1, suffix: "" });
    assert_wire_expr!(get_best_key("", "a/b", &face1), { scope: 1, suffix: "/b" });
    assert_wire_expr!(get_best_key("a", "", &face1), { scope: 1, suffix: "" });
    assert_wire_expr!(get_best_key("a", "/b", &face1), { scope: 1, suffix: "/b" });
    assert_wire_expr!(get_best_key("a/b", "", &face1), { scope: 1, suffix: "/b" });
    assert_wire_expr!(get_best_key("", "e", &face1), { scope: 0, suffix: "e" });
    assert_wire_expr!(get_best_key("", "a", &face2), { scope: 0, suffix: "a" });
    assert_wire_expr!(get_best_key("", "a/b", &face2), { scope: 2, suffix: "" });
    assert_wire_expr!(get_best_key("", "a/b/c", &face2), { scope: 3, suffix: "" });
    assert_wire_expr!(get_best_key("", "a/b/c/d", &face2), { scope: 3, suffix: "/d" });
    assert_wire_expr!(get_best_key("a", "", &face2), { scope: 0, suffix: "a" });
    assert_wire_expr!(get_best_key("a", "/b", &face2), { scope: 2, suffix: "" });
    assert_wire_expr!(get_best_key("a", "/d", &face2), { scope: 0, suffix: "a/d" });
    assert_wire_expr!(get_best_key("a/b", "", &face2), { scope: 2, suffix: "" });
}

#[test]
fn big_key_expr() {
    let router = new_router();

    let primitives = Arc::new(DummyPrimitives {});
    let face = router.new_session(primitives.clone());

    let root = zread!(router.tables.tables).data._get_root().clone();
    let key_expr = KeyExpr::new(vec!["a/"; 10000].concat() + "a").unwrap();
    let wire_expr = WireExpr::from(&**key_expr);
    register_expr(&router.tables, &mut face.state.clone(), 1, &wire_expr);
    let res = Resource::get_resource(&root, &key_expr).unwrap();
    root.get_best_key(&key_expr, face.state.id);
    res.get_best_key("/a", face.state.id + 1);
    Resource::get_matches(&face.tables.tables.read().unwrap().data, &key_expr);
}
