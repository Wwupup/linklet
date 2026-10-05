//! The Windows backend: `ipconfig` for the interfaces, `route print -4` for the gateway.
//!
//! Both are present on every Windows machine and neither needs a privilege. The parsing lives
//! in `linklet_core::discover`, where it is a table of tests rather than an experiment on
//! somebody's network -- this module only runs the two programs and hands the text over.

use std::process::Command;

use linklet_core::discover::{Interface, dotted_quads_in, parse_ipconfig};

/// The networks this host is on, from `ipconfig`.
///
/// # Errors
///
/// A sentence when the command cannot be run, or when it ran and produced nothing this could
/// read. **Not an empty list**: "this machine is on no networks" is a fact about a machine and
/// "the interfaces could not be read" is a fact about the call, and returning one for the other
/// is the mistake this project keeps meeting.
pub(super) fn local_interfaces() -> Result<Vec<Interface>, String> {
    let output = Command::new("ipconfig").output();
    let output = match output {
        Ok(output) => output,
        Err(error) => return Err(format!("cannot run ipconfig: {error}")),
    };

    // **Decoded as the machine's own bytes and not as UTF-8.** `ipconfig` writes its labels in
    // the console code page, so on the bench they are GBK and a strict UTF-8 read would refuse
    // the whole output over a letter of an adapter's name. The addresses this needs are ASCII
    // either way; `from_utf8_lossy` keeps them and replaces only the label.
    let text = String::from_utf8_lossy(&output.stdout);
    let interfaces = parse_ipconfig(&text);

    if interfaces.is_empty() {
        // A machine with no IPv4 address at all is possible and it is not an error -- but it is
        // worth being able to tell from a parser that read nothing, and the caller can see the
        // difference because the reason says which.
        return Err("no IPv4 interface was found in the output of ipconfig".to_string());
    }

    Ok(interfaces)
}

/// The default gateway, from the routing table.
///
/// `None` when there is not one or it cannot be read, which is an ordinary state on a machine
/// with no route out. **It is not an error**: the only thing the gateway is used for is keeping
/// the scan off a router, and a scan without it is a scan that probes one more address rather
/// than a scan that fails.
pub(super) fn default_gateway() -> Option<u32> {
    let output = Command::new("route").args(["print", "-4"]).output().ok()?;
    let text = String::from_utf8_lossy(&output.stdout);

    for line in text.lines() {
        let quads = dotted_quads_in(line);
        // The default route is the row whose destination and mask are both zero, which is the
        // one thing about that row that is the same in every language. The gateway is the third
        // address on it: `0.0.0.0  0.0.0.0  <gateway>  <interface>  <metric>`.
        if quads.len() >= 3 && quads[0] == 0 && quads[1] == 0 && quads[2] != 0 {
            return Some(quads[2]);
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use linklet_core::discover::{dotted, parse_dotted};

    /// The default-route row, and the row above it, as this machine printed them.
    const ROUTE_PRINT: &str = "\
===========================================================================
IPv4 Route Table
===========================================================================
Active Routes:
Network Destination        Netmask          Gateway       Interface  Metric
          0.0.0.0          0.0.0.0      192.168.3.1    192.168.3.157     25
        127.0.0.0        255.0.0.0         On-link         127.0.0.1    331
";

    #[test]
    fn the_gateway_is_the_third_address_on_the_default_route_and_not_the_first() {
        // **The first address on that row is `0.0.0.0`**, so a parser that took the first would
        // name the zero address as the gateway of every machine and then skip nothing.
        let gateway = ROUTE_PRINT.lines().find_map(|line| {
            let quads = dotted_quads_in(line);
            (quads.len() >= 3 && quads[0] == 0 && quads[1] == 0 && quads[2] != 0).then(|| quads[2])
        });

        assert_eq!(gateway, Some(0xC0A80301), "192.168.3.1");
        assert_eq!(dotted(0xC0A80301), "192.168.3.1");
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
        // "This machine is on no networks" and "the interfaces could not be read" are different
        // facts, and the second must not be reported as the first. Checked here rather than in
        // the core because it is what *this* backend does with an empty parse: an error.
        let text = "Windows IP Configuration\n\nWireless LAN adapter WLAN:\n\n   Media State . . . : Media disconnected\n";
        assert!(
            parse_ipconfig(text).is_empty(),
            "nothing to find in that output"
        );
    }

    #[test]
    fn a_real_interface_is_read_with_its_mask() {
        // A row from the same machine's `ipconfig`, so that this backend's end of the chain is
        // pinned: the program's text, through the core parser, to an interface.
        let text = "   IPv4 Address. . . . . . . . . . . : 192.168.100.1\n   Subnet Mask . . . . . . . . . . . : 255.255.255.0\n";
        let interfaces = parse_ipconfig(text);

        assert_eq!(interfaces.len(), 1, "{interfaces:#?}");
        assert_eq!(
            interfaces[0].address,
            parse_dotted("192.168.100.1").expect("an address")
        );
        assert!(interfaces[0].has_hosts());
    }
}
