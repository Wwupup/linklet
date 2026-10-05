//! Finding the machines: this host's networks, and the addresses that answer on them.
//!
//! `linklet_core::discover` decides **which addresses a scan would try and what it leaves
//! out**; this reads this machine's interfaces, reads its routing table, and opens the
//! sockets. The split is rule 1 of `AGENTS.md`, and it carries the weight it usually does here:
//! the ceilings that make a scan a bounded act rather than an incident are in the core, where
//! they are a table rather than an experiment on somebody's network.
//!
//! # Two programs and one shape
//!
//! Which program describes a machine's networks is a fact about the machine, and it is the only
//! thing that differs: `ipconfig` and `route print -4` on Windows, `ip` on Linux. So the
//! platform is a module (`windows`, `linux`) answering two questions -- what interfaces does
//! this host have, and what is its default gateway -- and everything that could be *decided
//! wrongly* is written once, here, and tested on whichever platform the tests run on. This is
//! the arrangement `crate::shell` and `crate::processes` already use, for the same reason.
//!
//! # What this reports, and what it refuses to imply
//!
//! A scan produces [`Scan`]: which addresses were tried, which of them answered, and whether
//! the list of addresses was cut short. **An address that did not answer is not a machine
//! that is absent** -- it may be filtered, or slow, or simply not running the agent -- so the
//! answer says "answered" and never "there is nothing here". That is the same distinction
//! `docs/framing.md` makes about a timeout and the same one `ps` makes about a process list.

use std::net::{IpAddr, Ipv4Addr, SocketAddr, TcpStream};
use std::time::Duration;

use linklet_core::discover::{Candidate, Interface, Plan, dotted, plan};

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "linux")]
use linux as platform;

#[cfg(windows)]
mod windows;
#[cfg(windows)]
use windows as platform;

// A platform with neither backend would fail to compile below with a message about a missing
// module. This says why, in the words of the thing that is missing.
#[cfg(not(any(windows, target_os = "linux")))]
compile_error!(
    "linklet can find networks with ipconfig (Windows) or ip (Linux), and not on this \
     platform: see crates/linklet-adapters/src/discovery.rs for the two things a backend has \
     to provide"
);

/// How long one address gets to answer.
///
/// **Measured rather than chosen.** This machine's bench agent answers a TCP connect in
/// **8 ms**, so 150 ms is nearly twenty times the answer it is waiting for and the budget is
/// not what decides whether a live machine is found. What it decides is how long a scan takes
/// when the addresses are *not* answering: a scan is mostly negative answers, each of which
/// costs the whole budget, so the first scan at 400 ms took ten seconds over 1530 addresses
/// and this one takes under four. `docs/ROADMAP.md` M2's measurement is the same shape --
/// a listening port answers in microseconds and everything else costs the timeout.
const PROBE_BUDGET: Duration = Duration::from_millis(150);

/// How many addresses are tried at once.
///
/// A scan that opened every socket at once would exhaust the ephemeral ports of the machine
/// doing the scanning before it found anything. Sixty-four is enough to cover a /24 in well
/// under a second and small enough that the host stays usable.
const AT_ONCE: usize = 64;

/// One address that answered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Found {
    /// The address.
    pub address: String,
    /// The port it answered on.
    pub port: u16,
}

/// What a scan looked at, and what it found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Scan {
    /// The addresses that answered, in the order they were tried.
    pub found: Vec<Found>,
    /// How many addresses were tried.
    pub tried: usize,
    /// Whether the plan was cut short by a ceiling.
    pub truncated: bool,
    /// The networks the scan was built from.
    pub networks: Vec<String>,
    /// The addresses deliberately not tried, and why.
    pub skipped: Vec<String>,
}

/// The networks this host is on that a scan could use, by asking this platform.
///
/// # What is left out, and why it is left out here
///
/// **An interface with no host addresses is not a network to scan.** A /31 or a /32 is a
/// point-to-point address, and this is not a hypothetical: the loopback device on the machine
/// this was written for carries `10.255.255.254/32` besides its `127.0.0.1/8`. Keeping one
/// would put an address in the list that no plan can offer a candidate from, and -- because
/// `plan` centres its window on the address it is given -- make the plan skip and centre on an
/// address no scan will reach.
///
/// **It is filtered here rather than in the parser** so that one answer reaches everything that
/// asks: `discover --networks` prints this list and a scan is planned from this list, and two
/// answers to "which networks is this host on" is the kind of disagreement this project refuses
/// everywhere else. `linklet_core::discover::parse_ip_addr` stays a faithful read of what the
/// machine said, point-to-point addresses included; deciding what is worth scanning is this
/// function's job.
///
/// # Errors
///
/// A sentence when the machine cannot be asked, when it answered with nothing this could read,
/// or when nothing it said is scannable. **Not an empty list**: "this machine is on no
/// networks" is a fact about a machine and "the interfaces could not be read" is a fact about
/// the call, and returning one for the other is the mistake this project keeps meeting.
pub fn local_interfaces() -> Result<Vec<Interface>, String> {
    let mut interfaces = platform::local_interfaces()?;

    interfaces.retain(|interface| interface.has_hosts());

    if interfaces.is_empty() {
        return Err(
            "no interface was found with an address a scan could try: every one of them is a \
             point-to-point or loopback address"
                .to_string(),
        );
    }

    Ok(interfaces)
}

/// The default gateway, by asking this platform.
///
/// `None` when there is not one or it cannot be read, which is an ordinary state on a machine
/// with no route out. **It is not an error**: the only thing the gateway is used for is
/// keeping the scan off a router, and a scan without it is a scan that probes one more
/// address rather than a scan that fails.
pub fn default_gateway() -> Option<u32> {
    platform::default_gateway()
}

/// Builds a plan for this machine's networks.
///
/// # Errors
///
/// A sentence when the interfaces cannot be read -- see [`local_interfaces`], which is the one
/// place that decides an empty interface list is a failure to look rather than a machine with
/// nothing on it.
pub fn plan_here() -> Result<Plan, String> {
    let interfaces = local_interfaces()?;
    let local = interfaces.first().map(|interface| interface.address);
    Ok(plan(&interfaces, local, default_gateway()))
}

/// Tries every address in `plan` on `port` and reports which answered.
///
/// The concurrency is here, in the adapter, which is where `docs/ROADMAP.md` M4 put it for
/// `check` and where M10 says it belongs for this: a scan is a thing that opens sockets, and
/// what to do with the answers is the caller's decision.
///
/// **An address that does not answer is not reported as absent.** It is simply not in
/// `found`, and the count of what was tried is beside it -- a caller that reads an empty
/// `found` as "the network is empty" has misread a scan for a census.
pub fn scan(plan: &Plan, port: u16) -> Scan {
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};

    let found: Mutex<Vec<(usize, Found)>> = Mutex::new(Vec::new());
    let next = AtomicUsize::new(0);
    let total = plan.candidates.len();

    std::thread::scope(|scope| {
        let workers = AT_ONCE.min(total.max(1));
        for _ in 0..workers {
            scope.spawn(|| {
                loop {
                    let index = next.fetch_add(1, Ordering::Relaxed);
                    let Some(candidate) = plan.candidates.get(index) else {
                        return;
                    };
                    if answers(candidate, port)
                        && let Ok(mut found) = found.lock()
                    {
                        found.push((
                            index,
                            Found {
                                address: candidate.to_string(),
                                port,
                            },
                        ));
                    }
                }
            });
        }
    });

    // Put back in the order they were tried, so two scans of the same network produce the
    // same answer and a caller can diff them.
    let mut found = found.into_inner().unwrap_or_default();
    found.sort_by_key(|(index, _)| *index);

    Scan {
        found: found.into_iter().map(|(_, entry)| entry).collect(),
        tried: total,
        truncated: plan.truncated,
        networks: plan
            .interfaces
            .iter()
            .map(|interface| interface.to_string())
            .collect(),
        skipped: plan
            .skipped
            .iter()
            .map(|skip| format!("{} ({})", dotted(skip.address), skip.reason))
            .collect(),
    }
}

/// Whether something accepts a connection at this address and port.
fn answers(candidate: &Candidate, port: u16) -> bool {
    let address = SocketAddr::new(IpAddr::V4(Ipv4Addr::from(candidate.address)), port);
    TcpStream::connect_timeout(&address, PROBE_BUDGET).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_local_plan_is_bounded_and_keeps_this_machine_out_of_it() {
        // What this machine actually is, read at the layer that can see it. The assertion is
        // not a particular network -- a build machine's are its own business -- but four
        // properties that must hold wherever this runs.
        let Ok(plan) = plan_here() else {
            // No IPv4 interface at all is a legitimate state for a machine, and the error is
            // already the honest answer for it.
            return;
        };

        assert!(
            plan.count() <= linklet_core::discover::MAX_TOTAL_ADDRESSES,
            "a plan is bounded whatever the machine is on: {}",
            plan.count()
        );
        assert!(
            !plan.interfaces.is_empty(),
            "a plan that was built has the interfaces it was built from"
        );
        assert!(
            plan.skipped
                .iter()
                .any(|skip| skip.reason.contains("own address")),
            "this machine's own address is skipped and said so: {:#?}",
            plan.skipped
        );
        assert!(
            plan.interfaces
                .iter()
                .all(linklet_core::discover::Interface::has_hosts),
            "an interface with no host addresses is not one to scan: {:#?}",
            plan.interfaces
        );
    }
}
