//! Client addresses as the server compares and counts them.

use std::net::{IpAddr, Ipv6Addr};

/// `ip` in one form: an IPv4 address mapped into IPv6 (`::ffff:192.0.2.1`, as a socket
/// listening on `[::]` reports an IPv4 peer) is the IPv4 address. Compare addresses only in
/// this form, or a trusted proxy on IPv4 never matches its mapped self.
pub(crate) fn canonical(ip: IpAddr) -> IpAddr {
    ip.to_canonical()
}

/// What a limit per address counts `ip` under: an IPv4 address on its own, an IPv6 address
/// by its /64, since one host is usually given a whole /64 and could otherwise take a new
/// address for each try. Only for counting: a trusted proxy is matched exactly.
pub(crate) fn limit_key(ip: IpAddr) -> IpAddr {
    match canonical(ip) {
        IpAddr::V6(v6) => {
            let network = u128::from(v6) & !u128::from(u64::MAX);
            IpAddr::V6(Ipv6Addr::from(network))
        }
        v4 => v4,
    }
}
