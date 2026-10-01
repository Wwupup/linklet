//! Everything that touches the outside world.
//!
//! This crate is allowed to be boring. Its job is to translate between the
//! pure types in `linklet-core` and the sockets, processes and files of a real
//! machine -- and to do nothing else. Decisions do not belong here; if a rule
//! about *what should happen* is written in this crate, it is in the one place
//! that cannot be tested without that machine.
//!
//! # Why the traits are not declared here
//!
//! The natural-looking arrangement is for this crate to define what a probe is
//! and for the core to call it. That arrangement is wrong, and the reason is
//! not stylistic: the core would then depend on this crate, so every test of a
//! rule would link the code that opens sockets. The core would keep compiling
//! and keep passing its tests right up until the day a network was needed.
//!
//! The arrow points `adapters -> core`, always. See `docs/rationale.md` rule 1.

#![forbid(unsafe_code)]

mod channel;
mod connection;
mod mcp;
mod processes;
mod system;
mod tcp;
mod transfer;

pub use channel::{HkdfChannel, OVERHEAD_BYTES, new_session_id};
pub use connection::{Connection, ConnectionError};
pub use mcp::{FALLBACK_PROTOCOL_VERSION, SERVER_NAME, SERVER_VERSION, serve};
pub use processes::list as list_processes;
pub use system::SystemProber;
pub use tcp::{MIN_BUDGET, TcpProbe, effective_budget};
pub use transfer::{TransferFailure, describe, digest_of_file, part_path, receive_body, send_body};
