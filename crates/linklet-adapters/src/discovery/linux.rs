//! The Linux backend: `ip` for both questions.
//!
//! # Why `ip` and not `/proc`
//!
//! The rest of this project prefers `/proc` where it can have it -- `processes/linux.rs` reads
//! it rather than parsing `ps`, and gives the reason: it is the machine-readable answer those
//! programs format. That argument does not carry here, and it is worth saying why rather than
//! leaving it as an inconsistency.
//!
//! There is no `/proc` file that answers "what IPv4 address does each interface have, and what
//! is its prefix length". `/proc/net/route` has the routes and their masks but never the local
//! address, and the address is the one thing a scan cannot do without: it is what the plan
//! centres its window on and what it must keep out of its own probe list. The kernel's
//! `/proc/net/fib_trie` does contain addresses, and it is a debugging dump whose nesting is not
//! a format anybody promises to keep -- a parser for it would be a parser for an implementation
//! detail of one kernel.
//!
//! `ip -o -4 addr show` is the answer a program is *meant* to be read from, and it has the
//! property that matters most here: **iproute2 does not translate its output.** The labels
//! `ipconfig` prints in Chinese are not a problem this has to solve, which is why the parsing
//! below is short.
//!
//! # What is deliberately not filtered
//!
//! Bridges and virtual interfaces -- `docker0`, `br-...`, `virbr0` -- are included, and the
//! reason is the same one that keeps `vEthernet` switches in the Windows list: **deciding which
//! of a machine's networks are "real" means reading their names**, and a name is a label. The
//! bench this project was developed against was reached through a Hyper-V virtual switch, which
//! a name-based rule would have skipped. What is excluded is excluded by address range
//! (loopback) or by shape (a network with no host addresses), and both are facts.

use std::process::Command;

use linklet_core::discover::{Interface, first_dotted_quad, parse_ip_addr};

/// The arguments that make `ip` print one address per line, IPv4 only.
///
/// `-o` is the whole point: without it an interface's address, mask, broadcast and lifetime are
/// four lines with labels in between, which is the shape this project has already paid twice to
/// avoid parsing. With it they are one line whose address token carries its own prefix length.
const ADDR_ARGS: [&str; 4] = ["-o", "-4", "addr", "show"];

/// The networks this host is on, from `ip -o -4 addr show`.
///
/// # Errors
///
/// A sentence when `ip` cannot be run, or when it ran and produced nothing this could read.
/// **Not an empty list**: "this machine is on no networks" is a fact about a machine and "the
/// interfaces could not be read" is a fact about the call.
pub(super) fn local_interfaces() -> Result<Vec<Interface>, String> {
    let output = Command::new("ip")
        .args(ADDR_ARGS)
        .output()
        .map_err(|error| format!("cannot run ip: {error}"))?;

    // `ip` writes its errors to stderr and exits non-zero, unlike `tasklist`, so a run that
    // failed is reported with what it said rather than as a machine with no interfaces.
    if !output.status.success() {
        let complaint = String::from_utf8_lossy(&output.stderr);
        return Err(format!(
            "ip -o -4 addr show exited {}: {}",
            output.status.code().unwrap_or(-1),
            complaint.trim()
        ));
    }

    let text = String::from_utf8_lossy(&output.stdout);
    let interfaces = parse_ip_addr(&text);

    if interfaces.is_empty() {
        // A machine whose only IPv4 address is loopback lands here, and it is not an error --
        // but it must be told apart from a parser that read nothing, and the reason says which.
        return Err(
            "no IPv4 interface was found in the output of ip: the only addresses are loopback \
             or unreadable"
                .to_string(),
        );
    }

    Ok(interfaces)
}

/// The default gateway, from `ip -4 route show default`.
///
/// **The first dotted quad on the line, and no word is read.** `default via 192.168.3.1 dev
/// eth1 proto kernel metric 25 onlink` -- the word `default` carries no address, so the first
/// address is the gateway whatever else the line says and in whatever order. A default route
/// with no `via` (`default dev eth0 scope link`) has no gateway, and comes back as `None`,
/// which is the ordinary answer on a machine with no route out.
///
/// `None` when there is not one or it cannot be read. **It is not an error**: the only thing
/// the gateway is used for is keeping the scan off a router, and a scan without it is a scan
/// that probes one more address rather than a scan that fails.
pub(super) fn default_gateway() -> Option<u32> {
    let output = Command::new("ip")
        .args(["-4", "route", "show", "default"])
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let text = String::from_utf8_lossy(&output.stdout);
    text.lines().find_map(first_dotted_quad)
}

#[cfg(test)]
mod tests {
    use super::*;
    use linklet_core::discover::{dotted, parse_dotted};

    /// Real `ip -4 route show default` output from the machine this was written on.
    const DEFAULT_ROUTE: &str = "default via 192.168.3.1 dev eth1 proto kernel metric 25 onlink \n";

    #[test]
    fn the_gateway_is_the_only_address_on_the_default_route() {
        // `default` is not an address, so the first dotted quad is the gateway -- which is the
        // whole of this parser, and the reason it does not need to know the word `via`.
        let gateway = DEFAULT_ROUTE.lines().find_map(first_dotted_quad);

        assert_eq!(gateway, parse_dotted("192.168.3.1"));
        assert_eq!(dotted(gateway.expect("a gateway")), "192.168.3.1");
    }

    #[test]
    fn a_default_route_with_no_gateway_has_no_gateway() {
        // `default dev eth0 scope link` is a real route on a machine with no router: the route
        // exists and there is nothing to keep off. Returning a made-up address here would put a
        // fabricated "skipped" line in a plan.
        let link_only = "default dev eth0 scope link \n";
        assert_eq!(link_only.lines().find_map(first_dotted_quad), None);
    }

    #[test]
    fn no_default_route_at_all_is_not_an_address() {
        // A machine with no route out prints nothing. `None` is the honest answer and the plan
        // simply has one fewer address to skip.
        assert_eq!("".lines().find_map(first_dotted_quad), None);
    }
}
