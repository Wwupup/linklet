//! The command line: read `argv`, call the core, print, set an exit code.
//!
//! The rule for this crate is that it contains no decisions. If there is an
//! `if` here that is not about formatting or about which exit code to use, the
//! decision has been written in the one place that is hardest to test.
//!
//! Exit codes, fixed now because an agent branches on them and they are a
//! contract once shipped:
//!
//! | code | meaning |
//! |------|---------|
//! | 0    | it did what was asked |
//! | 1    | it could not, and said why on stderr |
//! | 2    | the command line itself was wrong |
//!
//! Nothing is implemented yet -- see `docs/ROADMAP.md`, milestone M3.

fn main() {
    unimplemented!("the CLI arrives in milestone M3; the core comes first")
}
