//! Service lifetime tests shared by the console and tray entry points.
use super::*;
use linklet_adapters::{Connection, HkdfChannel};
use linklet_core::{
    channel::{EphemeralPublic, Handshake},
    frame::Kind,
    wire,
};
use std::net::TcpStream;

struct Scratch(PathBuf);
impl Scratch {
    fn new() -> Self {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        Self(std::env::temp_dir().join(format!(
            "linklet-lifetime-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        )))
    }
}
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
fn options() -> Options {
    Options {
        port: 0,
        log_path: None,
        no_log: true,
        reset: false,
    }
}

#[test]
fn stop_closes_listener_drains_accepted_request_and_keeps_lock_until_done() {
    let directory = Scratch::new();
    let agent = Agent::start_in(options(), &directory.0).expect("start");
    let info = agent.info().clone();
    assert_ne!(info.port, 0);
    assert_eq!(info.pairing_code.len(), 6);
    assert!(
        Agent::start_in(options(), &directory.0)
            .err()
            .expect("duplicate refused")
            .message
            .contains("running")
    );
    let token = agent.credentials.token().clone();
    let (stop, shutdown) = mpsc::channel();
    let (done, finished) = mpsc::channel();
    let worker = std::thread::spawn(move || {
        let _ = done.send(agent.serve(shutdown));
    });
    let address = std::net::SocketAddr::from(([127, 0, 0, 1], info.port));
    let mut connection = Connection::with_budget(
        TcpStream::connect(address).expect("connect"),
        Duration::from_secs(5),
    );
    let (public, pending) = HkdfChannel
        .propose(token.expose().as_bytes())
        .expect("propose");
    connection
        .write_frame(
            Kind::Hello,
            wire::handshake_to_json(public.as_bytes())
                .to_string()
                .as_bytes(),
        )
        .expect("hello");
    let reply = connection
        .read_frame(Kind::Hello)
        .expect("accepted handshake");
    let wire::Reply::Result(value) =
        wire::reply_from_json(&wire::parse_body(&reply).expect("JSON")).expect("reply")
    else {
        panic!("handshake refused")
    };
    let theirs =
        EphemeralPublic::from_bytes(wire::handshake_public_from_json(&value).expect("key"))
            .expect("public key");
    let mut session = pending.finish(&theirs).expect("session");
    stop.send(()).expect("stop");
    let deadline = std::time::Instant::now() + Duration::from_secs(3);
    while TcpStream::connect_timeout(&address, Duration::from_millis(100)).is_ok() {
        assert!(
            std::time::Instant::now() < deadline,
            "listener remained open after stop"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(
        matches!(finished.try_recv(), Err(mpsc::TryRecvError::Empty)),
        "active request was abandoned"
    );
    assert!(
        AgentCredentials::acquire(&directory.0, false).is_err(),
        "lock released while request remains active"
    );
    let sealed = session
        .seal(
            wire::request_to_json(&wire::Request::Identity)
                .to_string()
                .as_bytes(),
        )
        .expect("seal");
    connection
        .write_frame(Kind::Sealed, &sealed)
        .expect("request after stop");
    let reply = connection
        .read_frame(Kind::Sealed)
        .expect("drained response");
    let mut plaintext = Vec::new();
    session
        .open_into(&reply, &mut plaintext)
        .expect("open response");
    assert!(matches!(
        wire::reply_from_json(&wire::parse_body(&plaintext).expect("JSON")),
        Ok(wire::Reply::Result(_))
    ));
    drop(connection);
    finished
        .recv_timeout(Duration::from_secs(5))
        .expect("stopped")
        .expect("clean stop");
    worker.join().expect("worker");
    let credentials = AgentCredentials::acquire(&directory.0, false).expect("lock released");
    assert_eq!(credentials.code(), info.pairing_code);
}

#[test]
fn startup_binding_failure_and_disconnected_ui_release_credentials() {
    let directory = Scratch::new();
    let occupied = TcpListener::bind(("0.0.0.0", 0)).expect("occupied port");
    let mut config = options();
    config.port = occupied.local_addr().expect("address").port();
    let failure = Agent::start_in(config, &directory.0)
        .err()
        .expect("bind must fail");
    assert_eq!(failure.exit_code, 1);
    assert!(failure.message.contains("cannot listen on port"));
    let agent = Agent::start_in(options(), &directory.0).expect("failed startup released lock");
    let (stop, shutdown) = mpsc::channel();
    drop(stop);
    agent
        .serve(shutdown)
        .expect("disconnected UI stops service");
    drop(AgentCredentials::acquire(&directory.0, false).expect("stop released lock"));
}
