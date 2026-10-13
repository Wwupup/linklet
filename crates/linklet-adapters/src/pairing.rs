//! OPAQUE pairing. Registration is local; the code never travels over the socket.
use linklet_core::channel::Sealed;
use opaque_ke::{
    CipherSuite, ClientLogin, ClientLoginFinishParameters, ClientRegistration,
    ClientRegistrationFinishParameters, CredentialFinalization, CredentialRequest,
    CredentialResponse, ServerLogin, ServerLoginParameters, ServerRegistration, ServerSetup,
};
use rand::rngs::OsRng;

const CONTEXT: &[u8] = b"linklet pairing opaque-v1";
const ACCOUNT: &[u8] = b"linklet-agent";
struct Suite;
impl CipherSuite for Suite {
    type OprfCs = opaque_ke::Ristretto255;
    type KeyExchange = opaque_ke::TripleDh<opaque_ke::Ristretto255, sha2_legacy::Sha512>;
    type Ksf = argon2::Argon2<'static>;
}
fn failure(_: impl std::fmt::Display) -> String {
    "pairing authentication failed; check the six-digit code".into()
}
fn server_parameters() -> ServerLoginParameters<'static, 'static> {
    ServerLoginParameters {
        context: Some(CONTEXT),
        ..Default::default()
    }
}

/// A target's in-memory password record, rebuilt locally from its saved code on startup.
pub struct PairingServer {
    setup: ServerSetup<Suite>,
    record: ServerRegistration<Suite>,
}
impl PairingServer {
    /// Registers the six-digit secret locally using OPAQUE Ristretto255/SHA-512/Argon2id.
    pub fn new(code: &str) -> Result<Self, String> {
        linklet_core::pairing::validate_code(code)?;
        let setup = ServerSetup::<Suite>::new(&mut OsRng);
        let client =
            ClientRegistration::<Suite>::start(&mut OsRng, code.as_bytes()).map_err(failure)?;
        let server =
            ServerRegistration::<Suite>::start(&setup, client.message, ACCOUNT).map_err(failure)?;
        let registration = client
            .state
            .finish(
                &mut OsRng,
                code.as_bytes(),
                server.message,
                ClientRegistrationFinishParameters::default(),
            )
            .map_err(failure)?;
        Ok(Self {
            setup,
            record: ServerRegistration::finish(registration.message),
        })
    }
    /// Starts one attempt after the caller has consumed the agent-wide attempt budget.
    pub fn accept(&self, message: &[u8]) -> Result<(Vec<u8>, PairingPending), String> {
        let request = CredentialRequest::deserialize(message).map_err(failure)?;
        let result = ServerLogin::start(
            &mut OsRng,
            &self.setup,
            Some(self.record.clone()),
            request,
            ACCOUNT,
            server_parameters(),
        )
        .map_err(failure)?;
        Ok((
            result.message.serialize().to_vec(),
            PairingPending(result.state),
        ))
    }
}
/// Server-side attempt, consumed by the client's proof. No token can be released before finish.
pub struct PairingPending(ServerLogin<Suite>);
impl PairingPending {
    /// Verifies the client's proof and creates a dedicated encrypted pairing channel.
    pub fn finish(self, proof: &[u8]) -> Result<Box<dyn Sealed>, String> {
        let result = self
            .0
            .finish(
                CredentialFinalization::deserialize(proof).map_err(failure)?,
                server_parameters(),
            )
            .map_err(failure)?;
        crate::channel::pairing_session(&result.session_key, false).map_err(failure)
    }
}
/// Controller-side attempt, never reusable after finishing.
pub struct PairingClient(ClientLogin<Suite>);
impl PairingClient {
    /// Starts an attempt with a fresh blind and ephemeral key.
    pub fn start(code: &str) -> Result<(Vec<u8>, Self), String> {
        linklet_core::pairing::validate_code(code)?;
        let result = ClientLogin::<Suite>::start(&mut OsRng, code.as_bytes()).map_err(failure)?;
        Ok((result.message.serialize().to_vec(), Self(result.state)))
    }
    /// Authenticates the target, returning the client proof and encrypted pairing channel.
    pub fn finish(self, code: &str, response: &[u8]) -> Result<(Vec<u8>, Box<dyn Sealed>), String> {
        let parameters = ClientLoginFinishParameters {
            context: Some(CONTEXT),
            ..Default::default()
        };
        let result = self
            .0
            .finish(
                &mut OsRng,
                code.as_bytes(),
                CredentialResponse::deserialize(response).map_err(failure)?,
                parameters,
            )
            .map_err(failure)?;
        let channel =
            crate::channel::pairing_session(&result.session_key, true).map_err(failure)?;
        Ok((result.message.serialize().to_vec(), channel))
    }
}
