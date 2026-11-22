//! Port mapping on the home router for WebRTC listening: UPnP, else NAT-PMP.
//!
//! The router forwards the UDP port of the station. Then listeners behind a strict NAT,
//! such as most phones on mobile data, reach the station too. Without a router that
//! answers, nothing changes: STUN alone finds the address as before.

use igd_next::{PortMappingProtocol, SearchOptions};
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

/// The router keeps the mapping this long, in seconds, unless the station renews it.
const LEASE_S: u32 = 3600;
/// The station renews the mapping after this time.
const RENEW: Duration = Duration::from_secs(30 * 60);
/// After a failure, the station asks the router again after this time.
const RETRY: Duration = Duration::from_secs(5 * 60);
const DESCRIPTION: &str = "craftradio listeners";

/// The public address that the router forwards to the station, while it does.
pub(crate) type Mapped = Arc<Mutex<Option<SocketAddr>>>;

/// Starts the thread that maps the port of `local` and renews it. The thread removes the
/// mapping and ends when `running` turns false.
pub(crate) fn start(
    local: SocketAddr,
    mapped: Mapped,
    running: impl Fn() -> bool + Send + 'static,
) {
    // A station on this computer only, as in tests, has nothing to forward.
    if local.ip().is_loopback() || !local.is_ipv4() {
        return;
    }
    let _ = thread::Builder::new()
        .name("webrtc-upnp".into())
        .spawn(move || run(local, &mapped, &running));
}

/// A mapping that the router keeps, and how to remove it.
enum Mapping {
    Upnp(igd_next::Gateway, u16),
    Pmp,
}

fn run(local: SocketAddr, mapped: &Mapped, running: &dyn Fn() -> bool) {
    let mut current: Option<Mapping> = None;
    while running() {
        let result = map(local).map(|(g, a)| (Mapping::Upnp(g, a.port()), a));
        let wait = match result.or_else(|_| map_pmp(local, LEASE_S).map(|a| (Mapping::Pmp, a))) {
            Ok((mapping, external)) => {
                set(mapped, Some(external));
                current = Some(mapping);
                RENEW
            }
            Err(_) => {
                set(mapped, None);
                RETRY
            }
        };
        let until = Instant::now() + wait;
        while running() && Instant::now() < until {
            thread::sleep(Duration::from_millis(500));
        }
    }
    match current {
        Some(Mapping::Upnp(gateway, port)) => {
            let _ = gateway.remove_port(PortMappingProtocol::UDP, port);
        }
        // A lifetime of 0 removes the mapping.
        Some(Mapping::Pmp) => {
            let _ = map_pmp(local, 0);
        }
        None => {}
    }
    set(mapped, None);
}

fn set(mapped: &Mapped, value: Option<SocketAddr>) {
    if let Ok(mut m) = mapped.lock() {
        *m = value;
    }
}

/// Asks the router for a mapping: the same port if it is free, else any port.
fn map(local: SocketAddr) -> Result<(igd_next::Gateway, SocketAddr), String> {
    let mut options = SearchOptions::default();
    options.timeout = Some(Duration::from_secs(3));
    options.single_search_timeout = Some(Duration::from_secs(2));
    let gateway = igd_next::search_gateway(options).map_err(|e| e.to_string())?;
    let ip = gateway.get_external_ip().map_err(|e| e.to_string())?;
    // A router behind another NAT gives a private address. Such a mapping helps nobody.
    if !crate::webrtc::is_public(ip) {
        return Err(format!("the router has the private address {ip}"));
    }
    let port = match gateway.add_port(
        PortMappingProtocol::UDP,
        local.port(),
        local,
        LEASE_S,
        DESCRIPTION,
    ) {
        Ok(()) => local.port(),
        Err(_) => gateway
            .add_any_port(PortMappingProtocol::UDP, local, LEASE_S, DESCRIPTION)
            .map_err(|e| e.to_string())?,
    };
    Ok((gateway, SocketAddr::new(ip, port)))
}

/// The longest wait for one NAT-PMP answer.
const PMP_WAIT: std::time::Duration = Duration::from_secs(4);

/// Asks the default gateway for a NAT-PMP mapping of the same port, for `lifetime` seconds.
fn map_pmp(local: SocketAddr, lifetime: u32) -> Result<SocketAddr, String> {
    let gateway = natpmp::get_default_gateway().map_err(|e| e.to_string())?;
    map_pmp_at(gateway, local, lifetime)
}

fn map_pmp_at(
    gateway: std::net::Ipv4Addr,
    local: SocketAddr,
    lifetime: u32,
) -> Result<SocketAddr, String> {
    let mut pmp = natpmp::Natpmp::new_with(gateway).map_err(|e| e.to_string())?;
    pmp.send_public_address_request()
        .map_err(|e| e.to_string())?;
    let ip = match pmp_answer(&mut pmp)? {
        natpmp::Response::Gateway(g) => *g.public_address(),
        _ => return Err("the router gave no public address".into()),
    };
    if !crate::webrtc::is_public(std::net::IpAddr::V4(ip)) {
        return Err(format!("the router has the private address {ip}"));
    }
    pmp.send_port_mapping_request(natpmp::Protocol::UDP, local.port(), local.port(), lifetime)
        .map_err(|e| e.to_string())?;
    match pmp_answer(&mut pmp)? {
        natpmp::Response::UDP(m) => Ok(SocketAddr::new(ip.into(), m.public_port())),
        _ => Err("the router did not map the port".into()),
    }
}

/// Waits for the answer to the request in flight, at most [`PMP_WAIT`].
fn pmp_answer(pmp: &mut natpmp::Natpmp) -> Result<natpmp::Response, String> {
    let until = Instant::now() + PMP_WAIT;
    loop {
        match pmp.read_response_or_retry() {
            Ok(answer) => return Ok(answer),
            Err(natpmp::Error::NATPMP_TRYAGAIN) if Instant::now() < until => {
                thread::sleep(Duration::from_millis(50));
            }
            Err(e) => return Err(e.to_string()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{Ipv4Addr, UdpSocket};

    /// A router that answers NAT-PMP on this computer, as RFC 6886 describes.
    fn fake_router(public: Ipv4Addr, mapped_port: u16) -> thread::JoinHandle<Vec<u8>> {
        let socket = UdpSocket::bind((Ipv4Addr::LOCALHOST, 5351)).expect("port 5351 is free");
        socket.set_read_timeout(Some(Duration::from_secs(5))).ok();
        thread::spawn(move || {
            let mut asked = Vec::new();
            let mut buf = [0u8; 16];
            for _ in 0..2 {
                let Ok((n, from)) = socket.recv_from(&mut buf) else {
                    break;
                };
                let op = buf[1];
                asked.push(op);
                let mut reply = vec![0, 128 + op, 0, 0, 0, 0, 0, 1];
                if op == 0 {
                    reply.extend(public.octets());
                } else if n >= 12 {
                    reply.extend(&buf[4..6]);
                    reply.extend(mapped_port.to_be_bytes());
                    reply.extend(&buf[8..12]);
                }
                socket.send_to(&reply, from).ok();
            }
            asked
        })
    }

    #[test]
    fn a_nat_pmp_router_gives_the_public_address_and_port() {
        let router = fake_router(Ipv4Addr::new(81, 2, 69, 160), 40000);
        let local: SocketAddr = (Ipv4Addr::new(192, 168, 1, 20), 8000).into();
        let mapped = map_pmp_at(Ipv4Addr::LOCALHOST, local, LEASE_S);
        assert_eq!(mapped, Ok("81.2.69.160:40000".parse().unwrap()));
        assert_eq!(
            router.join().unwrap(),
            vec![0, 1],
            "address, then a UDP mapping"
        );
    }
}
