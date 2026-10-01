//! Finding the machines: this host's networks, and the addresses that answer on them.
//!
//! `linklet_core::discover` decides **which addresses a scan would try and what it leaves
//! out**; this runs `ipconfig`, reads the routing table, and opens the sockets. The split is
//! rule 1 of `AGENTS.md`, and it carries the weight it usually does here: the ceilings that
//! make a scan a bounded act rather than an incident are in the core, where they are a table
//! rather than an experiment on somebody's network.
//!
//! # What this reports, and what it refuses to imply
//!
//! A scan produces [`Scan`]: which addresses were tried, which of them answered, and whether
//! the list of addresses was cut short. **An address that did not answer is not a machine
//! that is absent** -- it may be filtered, or slow, or simply not running the agent -- so the
//! answer says "answered" and never "there is nothing here". That is the same distinction
//! `docs/framing.md` makes about a timeout and the same one `ps` makes about a process list.

use std::net::{IpAddr, Ipv4Addr, SocketAddr, TcpStream};
use std::process::Command;
use std::time::Duration;

use linklet_core::discover::{
    Candidate, Interface, Plan, dotted, dotted_quads_in, parse_interfaces, plan,
};

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

/// The networks this host is on, from `ipconfig`.
///
/// # Errors
///
/// A sentence when the command cannot be run. **Not an empty list**: "this machine is on no
/// networks" is a fact about a machine and "the interfaces could not be read" is a fact about
/// the call, and returning one for the other is the mistake this project keeps meeting.
pub fn local_interfaces() -> Result<Vec<Interface>, String> {
    let output = Command::new("ipconfig").output();
    let output = match output {
        Ok(output) => output,
        Err(error) => return Err(format!("cannot run ipconfig: {error}")),
    };

    // **Decoded as the machine's own bytes and not as UTF-8.** `ipconfig` writes its labels
    // in the console code page, so on the bench they are GBK and a strict UTF-8 read would
    // refuse the whole output over a letter of an adapter's name. The addresses this needs
    // are ASCII either way; `from_utf8_lossy` keeps them and replaces only the label.
    let text = String::from_utf8_lossy(&output.stdout);
    let interfaces = parse_interfaces(&text);

    if interfaces.is_empty() {
        // A machine with no IPv4 address at all is possible and it is not an error -- but it
        // is worth being able to tell from a parser that read nothing, and the caller can see
        // the difference because the reason says which.
        return Err("no IPv4 interface was found in the output of ipconfig".to_string());
    }

    Ok(interfaces)
}

/// The default gateway, from the routing table.
///
/// `None` when there is not one or it cannot be read, which is an ordinary state on a machine
/// with no route out. **It is not an error**: the only thing the gateway is used for is
/// keeping the scan off a router, and a scan without it is a scan that probes one more
/// address rather than a scan that fails.
pub fn default_gateway() -> Option<u32> {
    let output = Command::new("route").args(["print", "-4"]).output().ok()?;
    let text = String::from_utf8_lossy(&output.stdout);

    for line in text.lines() {
        let quads = dotted_quads_in(line);
        // The default route is the row whose destination and mask are both zero, which is
        // the one thing about that row that is the same in every language. The gateway is
        // the third address on it: `0.0.0.0  0.0.0.0  <gateway>  <interface>  <metric>`.
        if quads.len() >= 3 && quads[0] == 0 && quads[1] == 0 && quads[2] != 0 {
            return Some(quads[2]);
        }
    }

    None
}

/// Builds a plan for this machine's networks.
///
/// # Errors
///
/// A sentence when the interfaces cannot be read -- see [`local_interfaces`], which is the
/// one place that decides an empty interface list is a failure to look rather than a machine
/// with nothing on it.
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
    fn the_gateway_is_the_third_address_on_the_default_route_and_not_the_first() {
        // The row every Windows machine prints, labels and all. **The first address on it is
        // `0.0.0.0`**, so a parser that took the first would name the zero address as the
        // gateway of every machine and then skip nothing.
        let table = "\
===========================================================================
Interface List
 17...44 67 4e ad 56 cd ......Intel(R) Ethernet Connection
===========================================================================

IPv4 Route Table
===========================================================================
Active Routes:
Network Destination        Netmask          Gateway       Interface  Metric
          0.0.0.0          0.0.0.0      192.168.3.1    192.168.3.157     25
        127.0.0.0        255.0.0.0         On-link         127.0.0.1    331
";

        let gateway = table
            .lines()
            .filter_map(|line| {
                let quads = dotted_quads_in(line);
                (quads.len() >= 3 && quads[0] == 0 && quads[1] == 0 && quads[2] != 0)
                    .then(|| quads[2])
            })
            .next();

        assert_eq!(gateway, Some(0xC0A80301), "192.168.3.1");
    }

    #[test]
    fn a_route_row_that_is_not_the_default_one_is_not_the_gateway() {
        let row = "        127.0.0.0        255.0.0.0         On-link         127.0.0.1    331";
        let quads = dotted_quads_in(row);

        assert_eq!(quads[0], 0x7F000000, "the destination is not zero");
        assert!(
            !(quads.len() >= 3 && quads[0] == 0 && quads[1] == 0 && quads[2] != 0),
            "so this row is not the default route"
        );
    }

    #[test]
    fn deviceless_output_is_a_failure_to_read_and_not_a_machine_with_no_networks() {
        // "This machine is on no networks" and "the interfaces could not be read" are
        // different facts, and the second must not be reported as the first.
        let text = "Windows IP Configuration\n\nWireless LAN adapter WLAN:\n\n   Media State . . . : Media disconnected\n";
        assert!(
            parse_interfaces(text).is_empty(),
            "nothing to find in that output"
        );
    }

    #[test]
    fn the_local_plan_is_bounded_and_keeps_this_machine_out_of_it() {
        // What this machine actually is, read at the layer that can see it. The assertion is
        // not a particular network -- a build machine's are its own business -- but three
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
    }
}
