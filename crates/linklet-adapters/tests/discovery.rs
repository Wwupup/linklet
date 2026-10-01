//! Finding a machine, over real sockets.
//!
//! What is checked here is the part no pure test can see: that a scan **opens a connection**,
//! and that the address which answers is the one a listener is on. The plan -- which
//! addresses, which are skipped, where the ceiling is -- is a table in `linklet_core::discover`.
//!
//! # Why the loopback network is the fixture
//!
//! A scan needs a network with a machine on it, and this test cannot bring one. `127.0.0.0/8`
//! is that network and it is already here: it is a real network, `127.0.0.1` is a real
//! address on it, and a listener bound there answers a scan exactly as a machine on a LAN
//! does. That makes this the only offline test that can prove a scan opens a socket, which is
//! the one thing about it that a reader has to be able to trust.

use std::net::TcpListener;

use linklet_adapters::scan;
use linklet_core::discover::{Interface, plan};

/// The address the listeners in these tests bind to.
///
/// **Not `127.0.0.1`, and that is why it is a named constant.** The plan deliberately skips
/// this machine's own address -- a scan is looking for the *other* machines, and a caller that
/// wanted to know about this one would ask it directly -- so a fixture bound where the plan
/// refuses to look is never probed, which is what the first version of this test was. The
/// address next door is on the same network, answers the same way, and is inside the window.
const NEIGHBOUR: &str = "127.0.0.5";

/// A listener on a port the OS chose, and the port it got.
fn listening() -> (TcpListener, u16) {
    let listener = TcpListener::bind(format!("{NEIGHBOUR}:0")).expect("a port the OS picks");
    let port = listener
        .local_addr()
        .expect("a bound listener knows its address")
        .port();
    (listener, port)
}

/// A plan for loopback, which is a /8 and therefore far past the ceiling.
///
/// The point: this is also the only test that exercises the window on a **large** network
/// with a real machine inside it. `127.0.0.1` is the local address, so the window has to
/// contain it or the listener is never tried.
fn loopback_plan() -> linklet_core::discover::Plan {
    let interface = Interface::parse("127.0.0.1", "255.0.0.0").expect("loopback is a /8");
    plan(&[interface], Some(0x7F000001), None)
}

#[test]
fn a_scan_finds_the_machine_that_is_listening() {
    // **The claim the whole feature makes, and the only offline test that can make it.** A
    // scan is a thing that opens sockets; if this passes, the rest of discovery is arithmetic.
    let (listener, port) = listening();

    let result = scan(&loopback_plan(), port);

    assert!(
        result.found.iter().any(|found| found.address == NEIGHBOUR),
        "the listener at {NEIGHBOUR}:{port} was not found: {result:#?}"
    );
    assert_eq!(
        result.found[0].port, port,
        "the answer names the port that was scanned"
    );
    assert!(result.tried > 0);
    drop(listener);
}

#[test]
fn a_scan_of_a_port_nothing_is_on_finds_nothing_and_is_not_an_error() {
    // A scan is not a census: no answer means no answer, and the count of what was tried is
    // beside it so that a caller cannot read an empty list as an empty network. The port is
    // one nothing can be listening on because the OS just told us it was free.
    let (listener, port) = listening();
    drop(listener);

    let result = scan(&loopback_plan(), port);

    // Nothing on this port answers. Loopback refuses instantly, so this is fast -- and the
    // addresses are refused rather than filtered, which is the one kind of network that makes
    // a scan of a /8 cheap.
    assert!(result.tried > 0, "the scan looked at addresses");
    assert_eq!(
        result.found.len(),
        0,
        "nothing is listening on port {port}: {result:#?}"
    );
}

#[test]
fn the_window_holds_the_local_address_even_on_a_network_far_past_the_ceiling() {
    // What this protects: the first version of the planner walked up from the bottom of the
    // range, so on a /8 the window would have been 127.0.0.0/22 -- **which does not contain
    // 127.0.0.1's neighbours at all**, and a scan would have reported an empty network while
    // the machine it was looking for was four thousand addresses away.
    let plan = loopback_plan();
    let local = 0x7F000001;

    assert!(
        plan.truncated,
        "a /8 is past the ceiling and the plan says so"
    );
    assert!(
        plan.candidates
            .iter()
            .any(|candidate| candidate.address == local + 1),
        "the address beside this host is in the window"
    );
}
