//! PAKE interoperability, wrong secrets, replay and tampering.
use linklet_adapters::pairing::{PairingClient, PairingServer};
#[test]
fn both_roles_confirm_the_code_before_credentials_can_be_opened() {
    let server = PairingServer::new("000017").expect("registration");
    let (request, client) = PairingClient::start("000017").expect("start");
    let (reply, pending) = server.accept(&request).expect("accept");
    let (proof, mut client) = client.finish("000017", &reply).expect("server confirmed");
    let mut server_channel = pending.finish(&proof).expect("client confirmed");
    let mut sealed = Vec::new();
    server_channel
        .seal_into(b"test-only-credential", &mut sealed)
        .expect("seal");
    let mut clear = Vec::new();
    client.open_into(&sealed, &mut clear).expect("open");
    assert_eq!(clear, b"test-only-credential");
    assert!(
        client.open_into(&sealed, &mut clear).is_err(),
        "replay must fail"
    );

    let (request, wrong) = PairingClient::start("000018").expect("start");
    let (reply, _) = server.accept(&request).expect("accept");
    assert!(
        wrong.finish("000018", &reply).is_err(),
        "wrong code must fail"
    );

    let (request, client) = PairingClient::start("000017").expect("start");
    let (reply, pending) = server.accept(&request).expect("accept");
    let (mut proof, _) = client.finish("000017", &reply).expect("server confirmed");
    proof[0] ^= 1;
    assert!(
        pending.finish(&proof).is_err(),
        "no token release on a forged proof"
    );
}
