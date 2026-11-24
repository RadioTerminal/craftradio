use radio_server::Lockout;
use std::net::{IpAddr, Ipv4Addr};
use std::time::{Duration, Instant};

fn addr(s: &str) -> IpAddr {
    s.parse().unwrap()
}

fn secs(n: u64) -> Duration {
    Duration::from_secs(n)
}

#[test]
fn fifth_wrong_key_within_a_minute_locks_the_address_for_five_minutes() {
    let (l, t, a) = (Lockout::default(), Instant::now(), addr("203.0.113.9"));
    for i in 0..4 {
        l.failed(a, t + secs(i));
        assert!(!l.blocked(a, t + secs(i)));
    }
    l.failed(a, t + secs(4));
    assert!(l.blocked(a, t + secs(5)));
    assert!(l.blocked(a, t + secs(4 + 299)));
    assert!(!l.blocked(a, t + secs(4 + 301)));
}

#[test]
fn failures_spread_over_more_than_a_minute_do_not_lock() {
    let (l, t, a) = (Lockout::default(), Instant::now(), addr("203.0.113.10"));
    for i in 0..5 {
        let at = t + secs(i * 20);
        l.failed(a, at);
        assert!(!l.blocked(a, at), "blocked after failure {}", i + 1);
    }
    assert!(!l.blocked(a, t + secs(81)));
}

#[test]
fn one_address_locked_leaves_another_free() {
    let (l, t) = (Lockout::default(), Instant::now());
    let (locked, free) = (addr("203.0.113.11"), addr("203.0.113.12"));
    for i in 0..5 {
        l.failed(locked, t + secs(i));
    }
    l.failed(free, t + secs(5));
    assert!(l.blocked(locked, t + secs(6)));
    assert!(!l.blocked(free, t + secs(6)));
}

#[test]
fn old_entries_are_forgotten() {
    let (l, t) = (Lockout::default(), Instant::now());
    for i in 0..10_000u32 {
        l.failed(IpAddr::V4(Ipv4Addr::from(0x0a00_0000 + i)), t);
    }
    assert!(l.tracked() >= 10_000);
    l.failed(addr("203.0.113.13"), t + secs(301));
    assert!(l.tracked() < 100);
}

#[test]
fn forgetting_old_entries_keeps_a_locked_address_locked() {
    let (l, t) = (Lockout::default(), Instant::now());
    let (locked, other) = (addr("203.0.113.14"), addr("203.0.113.15"));
    for i in 0..5 {
        l.failed(locked, t + secs(i));
    }
    // Long after the failures left the window, but inside the lock.
    l.failed(other, t + secs(200));
    assert!(l.blocked(locked, t + secs(201)));
}
