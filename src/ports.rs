//! Deterministic port block allocation with a bind-and-connect probe. Std only.

use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr, TcpListener, TcpStream};
use std::path::Path;
use std::time::Duration;

const FNV_OFFSET: u64 = 0xcbf29ce484222325;
const FNV_PRIME: u64 = 0x100000001b3;

/// FNV-1a, 64 bit. Frozen: block placement must stay stable across releases.
pub fn fnv1a64(bytes: &[u8]) -> u64 {
    bytes.iter().fold(FNV_OFFSET, |hash, &b| {
        (hash ^ u64::from(b)).wrapping_mul(FNV_PRIME)
    })
}

/// Start ports of every aligned block of `count` ports inside `range` (inclusive).
pub fn blocks(range: (u16, u16), count: u16) -> Vec<u16> {
    let (lo, hi) = (u32::from(range.0), u32::from(range.1));
    let count = u32::from(count);
    if count == 0 {
        return Vec::new();
    }
    let first = lo.div_ceil(count) * count;
    (0..)
        .map(|i| first + i * count)
        .take_while(|start| start + count - 1 <= hi)
        .map(|start| start as u16)
        .collect()
}

/// Index of the block a repo and task prefer, out of `n` blocks.
pub fn preferred_index(repo: &Path, task: &str, n: usize) -> usize {
    use std::os::unix::ffi::OsStrExt;
    let mut key = repo.as_os_str().as_bytes().to_vec();
    key.push(0);
    key.extend_from_slice(task.as_bytes());
    (fnv1a64(&key) % n as u64) as usize
}

pub trait Prober {
    fn is_free(&self, port: u16) -> bool;
}

/// Free only if nothing listens on the port, on loopback or on a wildcard address.
pub struct BindProber;

const CONNECT_TIMEOUT: Duration = Duration::from_millis(100);

impl Prober for BindProber {
    fn is_free(&self, port: u16) -> bool {
        let v4 = SocketAddr::from((Ipv4Addr::LOCALHOST, port));
        let v6 = SocketAddr::from((Ipv6Addr::LOCALHOST, port));
        let v6_available = TcpListener::bind((Ipv6Addr::LOCALHOST, 0)).is_ok();

        // (a) bind, dropping each listener at once.
        if TcpListener::bind(v4).is_err() {
            return false;
        }
        if v6_available && TcpListener::bind(v6).is_err() {
            return false;
        }
        // (b) connect: sees wildcard listeners that a SO_REUSEADDR bind misses on macOS/BSD.
        if TcpStream::connect_timeout(&v4, CONNECT_TIMEOUT).is_ok() {
            return false;
        }
        if v6_available && TcpStream::connect_timeout(&v6, CONNECT_TIMEOUT).is_ok() {
            return false;
        }
        true
    }
}

/// First free block at or after the preferred one, wrapping around. A block is free when it
/// overlaps no `taken` range and every port in it probes free.
pub fn allocate(
    repo: &Path,
    task: &str,
    range: (u16, u16),
    count: u16,
    taken: &[(u16, u16)],
    prober: &dyn Prober,
) -> Option<u16> {
    let bs = blocks(range, count);
    if bs.is_empty() {
        return None;
    }
    let start = preferred_index(repo, task, bs.len());
    (0..bs.len())
        .map(|step| bs[(start + step) % bs.len()])
        .find(|&first| {
            let last = first + (count - 1);
            taken.iter().all(|&(lo, hi)| last < lo || first > hi)
                && (first..=last).all(|p| prober.is_free(p))
        })
}
