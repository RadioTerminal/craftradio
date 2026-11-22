//! UPnP port mapping on the home router for WebRTC listening.
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

fn run(local: SocketAddr, mapped: &Mapped, running: &dyn Fn() -> bool) {
    let mut current: Option<(igd_next::Gateway, u16)> = None;
    while running() {
        let wait = match map(local) {
            Ok((gateway, external)) => {
                set(mapped, Some(external));
                current = Some((gateway, external.port()));
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
    if let Some((gateway, port)) = current {
        let _ = gateway.remove_port(PortMappingProtocol::UDP, port);
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
