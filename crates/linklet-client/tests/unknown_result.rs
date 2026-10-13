//! A lost reply after delivery must never invite duplicate side effects.
use linklet_adapters::{Connection, HkdfChannel};
use linklet_client::{AgentAddress, CallError};
use linklet_core::{
    auth::Token,
    channel::{EphemeralPublic, Handshake},
    frame::Kind,
    wire,
};
use std::{net::TcpListener, time::Duration};

#[test]
fn delivered_command_without_reply_has_an_unknown_outcome() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("listener");
    let address = AgentAddress::new(listener.local_addr().expect("address").to_string())
        .expect("target")
        .with_token(Token::new("unknown-result-test-token").expect("token"));
    let peer = std::thread::spawn(move || {
        let (socket, _) = listener.accept().expect("accept");
        let mut connection = Connection::with_budget(socket, Duration::from_secs(5));
        let hello =
            wire::parse_body(&connection.read_frame(Kind::Hello).expect("hello")).expect("json");
        let public =
            EphemeralPublic::from_bytes(wire::handshake_public_from_json(&hello).expect("key"))
                .expect("key");
        let (ours, mut session) = HkdfChannel
            .accept(b"unknown-result-test-token", &public)
            .expect("handshake");
        connection
            .write_frame(
                Kind::Hello,
                wire::reply_result(wire::handshake_to_json(ours.as_bytes()))
                    .to_string()
                    .as_bytes(),
            )
            .expect("hello reply");
        let sealed = connection.read_frame(Kind::Sealed).expect("request");
        let mut bytes = Vec::new();
        session
            .open_into(&sealed, &mut bytes)
            .expect("authenticated request");
        assert!(matches!(
            wire::request_from_json(&wire::parse_body(&bytes).expect("json")),
            Ok(wire::Request::Run(_))
        ));
        // The command could now have modified the machine. Deliberately lose its reply.
    });
    let result = linklet_client::run(
        &address,
        &wire::RunRequest {
            cwd: None,
            command: "side effect".into(),
            timeout_seconds: 1,
        },
    );
    peer.join().expect("peer");
    assert!(
        matches!(result, Err(CallError::Unknown { .. })),
        "{result:?}"
    );
}
#[test]
fn delivered_manifest_without_acceptance_has_an_unknown_outcome() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("listener");
    let address = AgentAddress::new(listener.local_addr().expect("address").to_string())
        .expect("target")
        .with_token(Token::new("unknown-result-test-token").expect("token"));
    let peer = std::thread::spawn(move || {
        let (socket, _) = listener.accept().expect("accept");
        let mut connection = Connection::with_budget(socket, Duration::from_secs(5));
        let hello =
            wire::parse_body(&connection.read_frame(Kind::Hello).expect("hello")).expect("json");
        let public =
            EphemeralPublic::from_bytes(wire::handshake_public_from_json(&hello).expect("key"))
                .expect("key");
        let (ours, mut session) = HkdfChannel
            .accept(b"unknown-result-test-token", &public)
            .expect("handshake");
        connection
            .write_frame(
                Kind::Hello,
                wire::reply_result(wire::handshake_to_json(ours.as_bytes()))
                    .to_string()
                    .as_bytes(),
            )
            .expect("hello reply");
        let sealed = connection.read_frame(Kind::Sealed).expect("request");
        let mut bytes = Vec::new();
        session
            .open_into(&sealed, &mut bytes)
            .expect("authenticated request");
        assert!(matches!(
            wire::request_from_json(&wire::parse_body(&bytes).expect("json")),
            Ok(wire::Request::Push(_))
        ));
        // The command could now have modified the machine. Deliberately lose its reply.
    });
    let result = linklet_client::push(
        &address,
        std::path::Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/Cargo.toml")),
        "landed.bin",
    );
    peer.join().expect("peer");
    assert!(
        matches!(result, Err(CallError::Unknown { .. })),
        "{result:?}"
    );
}

#[test]
fn missing_local_source_is_a_path_error() {
    let address = AgentAddress::new("127.0.0.1:1").expect("address");
    let error = linklet_client::push(
        &address,
        std::path::Path::new("missing-linklet-source-file-9fe02"),
        "out",
    )
    .expect_err("missing");
    assert_eq!(
        error.diagnostic().code,
        linklet_core::result::ErrorCode::NotFound
    );
}
#[test]
fn committed_upload_with_lost_confirmation_has_an_unknown_outcome() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("listener");
    let address = AgentAddress::new(listener.local_addr().expect("address").to_string())
        .expect("target")
        .with_token(Token::new("unknown-result-test-token").expect("token"));
    let target = std::env::temp_dir().join(format!("linklet-lost-ack-{}.bin", std::process::id()));
    let received = target.clone();
    let peer = std::thread::spawn(move || {
        let (socket, _) = listener.accept().expect("accept");
        let mut connection = Connection::with_budget(socket, Duration::from_secs(5));
        let hello =
            wire::parse_body(&connection.read_frame(Kind::Hello).expect("hello")).expect("json");
        let public =
            EphemeralPublic::from_bytes(wire::handshake_public_from_json(&hello).expect("key"))
                .expect("key");
        let (ours, mut session) = HkdfChannel
            .accept(b"unknown-result-test-token", &public)
            .expect("handshake");
        connection
            .write_frame(
                Kind::Hello,
                wire::reply_result(wire::handshake_to_json(ours.as_bytes()))
                    .to_string()
                    .as_bytes(),
            )
            .expect("hello reply");
        let sealed = connection.read_frame(Kind::Sealed).expect("request");
        let mut bytes = Vec::new();
        session
            .open_into(&sealed, &mut bytes)
            .expect("authenticated request");
        let wire::Request::Push(manifest) =
            wire::request_from_json(&wire::parse_body(&bytes).expect("json")).expect("request")
        else {
            panic!("push");
        };
        let mut sealed = Vec::new();
        session
            .seal_into(
                wire::manifest_accepted(&manifest).to_string().as_bytes(),
                &mut sealed,
            )
            .expect("seal");
        connection
            .write_frame(Kind::Sealed, &sealed)
            .expect("acceptance");
        let result =
            linklet_adapters::receive_body(&mut connection, session.as_mut(), &manifest, &received)
                .expect("committed");
        assert_eq!(result.bytes, manifest.bytes);
        // The command could now have modified the machine. Deliberately lose its reply.
    });
    let result = linklet_client::push(
        &address,
        std::path::Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/Cargo.toml")),
        "landed.bin",
    );
    peer.join().expect("peer");
    assert_eq!(
        std::fs::read(&target).expect("committed file"),
        std::fs::read(concat!(env!("CARGO_MANIFEST_DIR"), "/Cargo.toml")).expect("source")
    );
    std::fs::remove_file(target).expect("cleanup");
    assert!(
        matches!(result, Err(CallError::Unknown { .. })),
        "{result:?}"
    );
}
