//! Everything that touches the outside world.
//!
//! This crate is allowed to be boring. Its job is to translate between the
//! pure types in `linklet-core` and the sockets, processes and files of a real
//! machine -- and to do nothing else. Decisions do not belong here; if a rule
//! about *what should happen* is written in this crate, it is in the one place
//! that cannot be tested without that machine.
//!
//! Nothing is implemented yet. The first thing to arrive here is the TCP probe
//! (milestone M2 in `docs/ROADMAP.md`), and it is shaped by the `Probe` trait
//! that `linklet-core` will define -- the core states what it needs, this
//! crate supplies it. That direction matters: it is what keeps the dependency
//! arrow pointing `adapters -> core`.

#![forbid(unsafe_code)]
