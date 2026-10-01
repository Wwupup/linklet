//! Finding machines: which networks this host is on, and which addresses a scan would try.
//!
//! This module is the **decision**; running `ipconfig` and opening the sockets is the
//! adapter's job. The split is rule 1 of `AGENTS.md`, and it matters more here than anywhere
//! else in this crate, because this is the one feature whose failure mode is not a wrong
//! answer but **a lot of traffic**.
//!
//! # The bound is the design
//!
//! `docs/ROADMAP.md` M10 says discovery is what every other call assumes: every command
//! names one `host:port`, and finding them is left to the person. "Scan the network" is the
//! obvious answer and it is not a feature until it has a number attached -- a /16 is 65,534
//! addresses, a /8 is sixteen million, and a tool that quietly tried all of them would be a
//! port scanner that somebody else has to explain.
//!
//! So there are **two ceilings and both are reported**:
//!
//! - [`MAX_ADDRESSES_PER_NETWORK`] caps one network, and a network larger than it is cut to
//!   the part that contains the local address. [`Plan::truncated`] says so.
//! - [`MAX_TOTAL_ADDRESSES`] caps the whole plan, and a plan that hits it says so too.
//!
//! A plan is therefore a promise a caller can read: these addresses, out of a network of
//! this size, and here is what was left out. Nothing is scanned that is not in it.
//!
//! # What is deliberately not scanned
//!
//! The local address and the default gateway. The first is this machine -- the question
//! "does something answer here" is answered by the fact that the command ran -- and the
//! second is a router, which is to say a device that was not asked to be part of this. Both
//! are listed in [`Plan::skipped`] with their reason rather than being silently dropped,
//! because "I did not look there" and "nothing was there" are the same mistake this whole
//! milestone keeps meeting.

use std::fmt;

/// The most addresses a plan will scan on one network.
///
/// A /24 is 254 hosts, which is what a LAN is; a /22 is 1022 and is what a larger office
/// looks like. The ceiling is that second number and **not the size of a /20**, because a /20
/// is what a Hyper-V switch hands out -- this machine has one -- and sweeping four thousand
/// addresses for an agent that is on one of them is a scan nobody asked for. Beyond the
/// ceiling the window is the part of the network nearest this host, which is where the
/// machines are.
pub const MAX_ADDRESSES_PER_NETWORK: usize = 1024;

/// The most addresses a plan will scan in total.
///
/// Across every interface: a machine with a wired and a wireless adapter is on two networks
/// and the plan is the union. This is the number that keeps a plan a few seconds of traffic
/// rather than an incident -- and it was four thousand until the first real scan took ten
/// seconds and made the point that the ceiling is about time, not about addresses.
pub const MAX_TOTAL_ADDRESSES: usize = 1600;

/// One IPv4 address, and the mask that says which network it is on.
///
/// Both are `u32` rather than dotted text, because everything this does with them is
/// arithmetic: the network is a bitwise and, the broadcast is an or against the inverse, and
/// a range is a subtraction. Text that happens to look like an address is parsed once, in
/// [`Interface::parse`], and never again.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Interface {
    /// This machine's address on that network.
    pub address: u32,
    /// The subnet mask.
    pub mask: u32,
}

impl Interface {
    /// Reads an address and a mask from their dotted form.
    ///
    /// # Errors
    ///
    /// A sentence naming which of the two was not a dotted quad, or that the mask is not a
    /// mask. **A non-contiguous mask is refused rather than tolerated**: `255.0.255.0` has a
    /// network and a broadcast that no arithmetic here produces, and treating it as a
    /// contiguous one would scan an address range the operator never described.
    pub fn parse(address: &str, mask: &str) -> Result<Self, String> {
        let address =
            parse_dotted(address).ok_or_else(|| format!("{address:?} is not an address"))?;
        let mask = parse_dotted(mask).ok_or_else(|| format!("{mask:?} is not a subnet mask"))?;

        if !is_contiguous_mask(mask) {
            // The dotted form in the message, not the number: the caller typed dots and a
            // refusal that answers in decimal would be a refusal about something else.
            return Err(format!(
                "{} is not a subnet mask: its bits are not all ones followed by all zeros",
                dotted(mask)
            ));
        }

        Ok(Self { address, mask })
    }

    /// The address of the network itself.
    pub fn network(&self) -> u32 {
        self.address & self.mask
    }

    /// The highest address, which is the broadcast.
    pub fn broadcast(&self) -> u32 {
        self.address | !self.mask
    }

    /// How many addresses the network holds, counting the network and the broadcast.
    pub fn size(&self) -> u64 {
        u64::from(self.broadcast() - self.network()) + 1
    }

    /// The usable addresses: everything between the network and the broadcast.
    ///
    /// A /31 and a /32 have none by the ordinary rule -- the two-address point-to-point case
    /// RFC 3021 makes usable, and the single-host case -- and both come back empty rather
    /// than as an off-by-one range, because a plan that offered the network address as a
    /// host would probe an address the operator does not use.
    pub fn hosts(&self) -> std::ops::Range<u32> {
        if self.mask >= u32::MAX - 1 {
            return 0..0;
        }
        (self.network() + 1)..self.broadcast()
    }
}

impl fmt::Display for Interface {
    /// The address and mask as a person writes them, which is also how a refusal names them.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}/{}.{}.{}.{}",
            dotted(self.address),
            (self.mask >> 24) & 0xff,
            (self.mask >> 16) & 0xff,
            (self.mask >> 8) & 0xff,
            self.mask & 0xff
        )
    }
}

/// One address a scan will try, and why it is in the plan.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Candidate {
    /// The address.
    pub address: u32,
    /// Which of this machine's networks it came from.
    pub interface: Interface,
}

impl fmt::Display for Candidate {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&dotted(self.address))
    }
}

/// An address the plan deliberately will not try.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Skipped {
    /// The address.
    pub address: u32,
    /// Why it is not being scanned, in the words a reader needs.
    pub reason: &'static str,
}

/// What a scan would do, and what it would leave out.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    /// The addresses to try, in the order they will be tried.
    pub candidates: Vec<Candidate>,
    /// The addresses that were deliberately not included.
    pub skipped: Vec<Skipped>,
    /// Whether a network was cut down to fit a ceiling.
    pub truncated: bool,
    /// The interfaces the plan was built from.
    pub interfaces: Vec<Interface>,
}

impl Plan {
    /// The number of addresses the plan will try.
    pub fn count(&self) -> usize {
        self.candidates.len()
    }
}

/// Builds the plan for the networks this host is on.
///
/// `local` and `gateway` are the addresses that must not be probed, given as interfaces
/// rather than found here because they are facts about a running machine.
///
/// # The order, and why it is this one
///
/// Networks are planned in the order they were handed over, and inside one they run from the
/// low address upwards -- so a scan that is interrupted covered a predictable part of the
/// network rather than a scattered one. The local and gateway addresses are removed before
/// the ceilings are applied, so that the number of addresses actually tried is the number
/// the plan reports.
pub fn plan(interfaces: &[Interface], local: Option<u32>, gateway: Option<u32>) -> Plan {
    let mut candidates: Vec<Candidate> = Vec::new();
    let mut skipped: Vec<Skipped> = Vec::new();
    let mut truncated = false;

    for interface in interfaces {
        let hosts = interface.hosts();
        let available = hosts.clone().count();

        // **The window is centred on this host, not taken from the bottom of the network.**
        // The first version walked up from the network address, which on this machine meant
        // scanning 172.18.112.0/22 of a Hyper-V /20 while the agent being looked for sat at
        // the *other* end of it -- a scan that reported "nothing found" about 1024 addresses
        // when the machine was in the 3070 it never tried. The machines a scan is looking for
        // are the ones beside this host.
        let (start, end) = window(interface, hosts, available);
        if available > MAX_ADDRESSES_PER_NETWORK {
            // **Said here, and not only at the total ceiling.** A network with more addresses
            // than the window holds was cut, whatever the rest of the plan goes on to do --
            // and a plan that reported itself complete over a /20 it had looked at a quarter
            // of would be the most dangerous answer this module can give. Found by the two
            // ceiling tests, which went red the moment the window stopped running to the end
            // of the range.
            truncated = true;
        }

        for address in start..end {
            if Some(address) == local {
                skipped.push(Skipped {
                    address,
                    reason: "this machine's own address",
                });
                continue;
            }
            if Some(address) == gateway {
                skipped.push(Skipped {
                    address,
                    reason: "the default gateway, which was not asked to be part of this",
                });
                continue;
            }

            if candidates.len() >= MAX_TOTAL_ADDRESSES {
                truncated = true;
                break;
            }

            candidates.push(Candidate {
                address,
                interface: *interface,
            });
        }

        if candidates.len() >= MAX_TOTAL_ADDRESSES {
            truncated = true;
            break;
        }
    }

    let interfaces = interfaces.to_vec();
    Plan {
        candidates,
        skipped,
        truncated,
        interfaces,
    }
}

/// The addresses of one interface that a plan will consider.
///
/// The whole usable range when it fits under the ceiling, and otherwise the ceiling's worth
/// that contains this host's address. The window is pushed towards the local address rather
/// than to either edge, so a /16 is scanned around the machine doing the scanning instead of
/// at its bottom. **This is the correction the first real scan needed**: walking up from the
/// network address meant that on a Hyper-V /20 the plan covered 172.18.112.0/22 while the
/// agent it was looking for sat in the 3070 addresses it never tried.
fn window(interface: &Interface, hosts: std::ops::Range<u32>, available: usize) -> (u32, u32) {
    if available <= MAX_ADDRESSES_PER_NETWORK {
        return (hosts.start, hosts.end);
    }

    let local = interface.address;
    if !hosts.contains(&local) {
        // The caller passed an address that is not on this interface -- one address per
        // interface, and it gave the wrong one. The bottom of the range is a definite part of
        // a network rather than an arbitrary one.
        return (hosts.start, hosts.start + MAX_ADDRESSES_PER_NETWORK as u32);
    }

    let half = (MAX_ADDRESSES_PER_NETWORK / 2) as u32;
    let mut start = local.saturating_sub(half).max(hosts.start);
    let mut end = start + MAX_ADDRESSES_PER_NETWORK as u32;
    if end > hosts.end {
        end = hosts.end;
        start = end
            .saturating_sub(MAX_ADDRESSES_PER_NETWORK as u32)
            .max(hosts.start);
    }

    (start, end)
}

/// Reads an address from its dotted form.
///
/// **Four parts and no more.** `1.2.3` and `1.2.3.4.5` are both refused: a resolver would
/// accept the first as shorthand and a scan that did would probe an address the operator did
/// not write.
pub fn parse_dotted(text: &str) -> Option<u32> {
    let parts: Vec<&str> = text.trim().split('.').collect();
    if parts.len() != 4 {
        return None;
    }

    let mut value = 0u32;
    for part in parts {
        // Rust's `u8` parse refuses `+1` and ` 1` and accepts nothing but digits, which is
        // what an address is.
        let octet: u8 = part.parse().ok()?;
        value = (value << 8) | u32::from(octet);
    }
    Some(value)
}

/// An address in its dotted form.
pub fn dotted(address: u32) -> String {
    format!(
        "{}.{}.{}.{}",
        (address >> 24) & 0xff,
        (address >> 16) & 0xff,
        (address >> 8) & 0xff,
        address & 0xff
    )
}

/// Whether a mask is all ones followed by all zeros.
///
/// The property that makes `network` and `broadcast` mean what they say. `255.255.0.255` is
/// a mask whose zero has a one above it, and no arithmetic here produces a sensible range
/// from it.
fn is_contiguous_mask(mask: u32) -> bool {
    // How many leading ones, and are they all of them? That is the whole test, written as
    // the arithmetic rather than as a list of the masks that occur in practice -- and
    // `255.255.0.255` is exactly the case a list gets wrong, which is the case that would
    // scan a range nobody described. Found by the test below, not by reading: the first
    // version of this asked whether the inverse plus one was a power of two, which says
    // `255.255.0.255` has the right shape.
    //
    // The zero mask is the one edge a shift cannot express -- there is no "shift by 32" --
    // and it is a mask a misconfigured adapter can carry.
    let leading = mask.leading_ones();
    mask == if leading == 0 {
        0
    } else {
        u32::MAX << (32 - leading)
    }
}

/// The interfaces described by `ipconfig` output.
///
/// # Why this reads addresses and not labels
///
/// `ipconfig` labels every line in the language the machine is set to -- `IPv4 Address` on
/// this machine, something else on a Chinese or German one -- and this project has already
/// paid once for reading a localised message. It does not need to read them: an address is
/// four dotted octets and a mask is a mask, in every language, on one line each and adjacent
/// to each other. The parser therefore looks for a dotted quad that is an address followed
/// by a dotted quad that is a mask, and never looks at a word.
///
/// That is also why a disconnected adapter is harmless: it has no address line, so it
/// contributes nothing, and the ones that do contribute are the ones this host is on.
pub fn parse_interfaces(text: &str) -> Vec<Interface> {
    let mut interfaces = Vec::new();
    let mut pending: Option<u32> = None;

    for line in text.lines() {
        let Some(value) = first_dotted_quad(line) else {
            continue;
        };

        // A mask ends a pair. `is_contiguous_mask` is the test that tells the two lines
        // apart without reading either one's label: an address is any four octets, and a
        // mask is a shape.
        if is_contiguous_mask(value) && pending.is_some() {
            let address = pending.take().expect("checked above");
            interfaces.push(Interface {
                address,
                mask: value,
            });
            continue;
        }

        // The first dotted quad on a line that carries several: `ipconfig` writes one value
        // per line except for a gateway list, which is not an address of ours.
        pending = Some(value);
    }

    interfaces
}

/// The first dotted quad on a line, if the line has one.
///
/// The rest of the line is ignored, which is the point: it is a label in some language and
/// sometimes a `%17` scope suffix after an IPv6 address, and neither is this parser's
/// business.
///
/// **Public because the routing table is read with it too.** `route print -4` labels its
/// columns in the machine's language and puts the default gateway in a column whose position
/// moves; what it cannot move is that the row for the default route contains dotted quads and
/// that the gateway is the one after the interface address. Reading addresses and not labels
/// is the same technique in both places, so it is one function rather than two.
pub fn first_dotted_quad(line: &str) -> Option<u32> {
    for word in line.split_whitespace() {
        if word.matches('.').count() == 3
            && let Some(value) = parse_dotted(word)
        {
            return Some(value);
        }
    }
    None
}

/// Every dotted quad on a line, in the order they appear.
///
/// The routing table needs more than the first one: the default route's row is
/// `0.0.0.0  0.0.0.0  <gateway>  <interface>  <metric>`, and it is the *third* that is the
/// gateway. Taking the first would name `0.0.0.0` as the gateway of every machine.
pub fn dotted_quads_in(line: &str) -> Vec<u32> {
    line.split_whitespace()
        .filter(|word| word.matches('.').count() == 3)
        .filter_map(parse_dotted)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Real `ipconfig` output from this machine, labels and all.
    ///
    /// Included whole rather than trimmed to the interesting lines, because the lines that
    /// are *not* interesting are the test: every one of them is a label in a language this
    /// parser must not read, and a truncated fixture would not prove that it does not.
    const IPCONFIG: &str = "\
Windows IP Configuration

Ethernet adapter vEthernet (LAN-INTERNAL):

   Connection-specific DNS Suffix  . : 
   Link-local IPv6 Address . . . . . : fe80::ef67:6265:6181:9895%49
   IPv4 Address. . . . . . . . . . . : 192.168.100.1
   Subnet Mask . . . . . . . . . . . : 255.255.255.0
   Default Gateway . . . . . . . . . : 

Ethernet adapter Ethernet:

   Connection-specific DNS Suffix  . : 
   IPv6 Address. . . . . . . . . . . : 240e:3bc:3015:fdc1:24fb:6505:b71b:2
   IPv4 Address. . . . . . . . . . . : 192.168.3.157
   Subnet Mask . . . . . . . . . . . : 255.255.255.0
   Default Gateway . . . . . . . . . : fe80::1%17
                                       192.168.3.1

Wireless LAN adapter Wireless Network Connection:

   Media State . . . . . . . . . . . : Media disconnected
   Connection-specific DNS Suffix  . : 
";

    #[test]
    fn the_interfaces_are_read_without_reading_a_single_label() {
        // **The whole parser, against real output.** Two adapters are on networks and the
        // third is disconnected; the labels are English here and would be Chinese on the
        // bench, and the answer does not change because no label is looked at.
        let interfaces = parse_interfaces(IPCONFIG);

        assert_eq!(interfaces.len(), 2, "{interfaces:#?}");
        assert_eq!(
            interfaces[0].address,
            parse_dotted("192.168.100.1").expect("an address")
        );
        assert_eq!(
            interfaces[0].mask,
            parse_dotted("255.255.255.0").expect("a mask")
        );
        assert_eq!(
            interfaces[1].address,
            parse_dotted("192.168.3.157").expect("an address")
        );
    }

    #[test]
    fn an_address_without_a_mask_is_not_an_interface() {
        // A half-read pair is not a network, and inventing a mask for it would be inventing
        // the address range the plan then scans.
        let text = "   IPv4 Address. . . . . . . . . . . : 10.0.0.5\n   Media disconnected\n";
        assert!(parse_interfaces(text).is_empty());
    }

    #[test]
    fn a_mask_that_is_not_contiguous_is_refused_by_name() {
        // No arithmetic here produces a sensible range from it, and treating it as
        // contiguous would scan an address range the operator never described.
        let error = Interface::parse("10.0.0.5", "255.0.255.0").expect_err("not a mask");
        assert!(error.contains("subnet mask"), "{error}");
        assert!(error.contains("255.0.255.0"), "{error}");
    }

    #[test]
    fn a_network_and_its_hosts_are_the_arithmetic_they_say() {
        let interface = Interface::parse("192.168.100.1", "255.255.255.0").expect("a /24");

        assert_eq!(
            interface.network(),
            parse_dotted("192.168.100.0").expect("an address")
        );
        assert_eq!(
            interface.broadcast(),
            parse_dotted("192.168.100.255").expect("an address")
        );
        assert_eq!(interface.size(), 256);
        assert_eq!(interface.hosts().count(), 254);
        assert_eq!(
            interface.hosts().start,
            parse_dotted("192.168.100.1").expect("an address")
        );
        assert_eq!(
            interface.hosts().end,
            parse_dotted("192.168.100.255").expect("an address"),
            "the broadcast is not a host"
        );
    }

    #[test]
    fn a_point_to_point_network_has_no_hosts_to_offer() {
        // A /31 and a /32 have no usable host range by the ordinary rule. Coming back empty
        // is the honest answer; an off-by-one range would probe the network address.
        let pair = Interface::parse("10.0.0.1", "255.255.255.254").expect("a /31");
        assert_eq!(pair.hosts().count(), 0);

        let single = Interface::parse("10.0.0.1", "255.255.255.255").expect("a /32");
        assert_eq!(single.hosts().count(), 0);
    }

    #[test]
    fn the_local_address_and_the_gateway_are_listed_as_skipped_and_not_silently_dropped() {
        // **"I did not look there" and "nothing was there" must not be the same answer.**
        let interface = Interface::parse("192.168.100.10", "255.255.255.0").expect("a /24");
        let local = parse_dotted("192.168.100.10").expect("an address");
        let gateway = parse_dotted("192.168.100.1").expect("an address");

        let plan = plan(&[interface], Some(local), Some(gateway));

        assert_eq!(
            plan.count(),
            252,
            "254 hosts less the two that were removed"
        );
        assert!(!plan.truncated);
        assert_eq!(plan.skipped.len(), 2, "{:#?}", plan.skipped);
        assert!(
            plan.skipped.iter().any(|skip| skip.address == local),
            "the local address is in the skipped list"
        );
        assert!(
            plan.skipped
                .iter()
                .any(|skip| skip.address == gateway && skip.reason.contains("gateway")),
            "and so is the gateway, with the reason"
        );
        assert!(
            !plan.candidates.iter().any(|c| c.address == local),
            "neither is in the plan"
        );
    }

    #[test]
    fn a_network_larger_than_the_ceiling_is_cut_and_says_so() {
        // A /16 is 65,534 addresses; the ceiling is a thousand and the plan says it was cut.
        // **A plan that did not say so would be a scan that looked like a whole network and
        // was a fifth of a percent of one.**
        let interface = Interface::parse("10.0.0.5", "255.255.0.0").expect("a /16");

        let plan = plan(&[interface], None, None);

        assert_eq!(plan.count(), MAX_ADDRESSES_PER_NETWORK);
        assert!(plan.truncated);
        assert_eq!(
            plan.candidates[0].address,
            parse_dotted("10.0.0.1").expect("an address"),
            "the window starts at the bottom of the network, where the local address is"
        );
    }

    #[test]
    fn the_total_ceiling_stops_a_machine_that_is_on_many_networks() {
        // Five networks of about a thousand each, so the **total** ceiling is what binds and
        // not the per-network one. Written with /22s because five /24s is 1270 addresses --
        // under the total, so a test built from /24s would pass while asserting a ceiling it
        // never reached. That is what the first version of this did.
        let interfaces: Vec<Interface> = (1..=5)
            .map(|third| {
                Interface::parse(&format!("10.0.{third}.1"), "255.255.252.0").expect("a /22")
            })
            .collect();

        let plan = plan(&interfaces, None, None);

        assert_eq!(
            plan.count(),
            MAX_TOTAL_ADDRESSES,
            "the total ceiling is the one that binds"
        );
        assert!(plan.truncated);
    }

    #[test]
    fn a_per_network_ceiling_binds_before_the_total_one() {
        // One /16 offers 65,534 addresses: the per-network ceiling stops it long before the
        // total could, and the plan still says it was cut.
        let interface = Interface::parse("10.0.0.1", "255.255.0.0").expect("a /16");

        let plan = plan(&[interface], None, None);

        assert_eq!(plan.count(), MAX_ADDRESSES_PER_NETWORK);
        assert!(
            plan.count() < MAX_TOTAL_ADDRESSES,
            "this is the per-network ceiling, not the total"
        );
        assert!(plan.truncated);
    }

    #[test]
    fn an_address_is_four_parts_and_nothing_else() {
        // A resolver accepts shorthand; a scan that did would probe an address the operator
        // did not write.
        assert_eq!(parse_dotted("1.2.3.4"), Some(0x01020304));
        assert_eq!(parse_dotted(" 1.2.3.4 "), Some(0x01020304), "trimmed");
        assert_eq!(parse_dotted("1.2.3"), None);
        assert_eq!(parse_dotted("1.2.3.4.5"), None);
        assert_eq!(parse_dotted("1.2.3.256"), None);
        assert_eq!(parse_dotted("1.2.3.-1"), None);
        assert_eq!(parse_dotted("::1"), None);
        assert_eq!(dotted(0x01020304), "1.2.3.4");
    }
}
