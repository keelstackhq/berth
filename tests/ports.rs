use berth::ports::{BindProber, Prober, allocate, blocks, fnv1a64, preferred_index};
use std::net::TcpListener;
use std::path::Path;

#[test]
fn fnv_vectors() {
    assert_eq!(fnv1a64(b""), 0xcbf29ce484222325);
    assert_eq!(fnv1a64(b"a"), 0xaf63dc4c8601ec8c);
    assert_eq!(fnv1a64(b"foobar"), 0x85944171f73967e8);
    assert_eq!(fnv1a64(b"/abs/hull\0fix-login"), 0x80d6449706dabb4f);
}

#[test]
fn preferred_index_vector() {
    assert_eq!(
        preferred_index(Path::new("/abs/hull"), "fix-login", 1000),
        751
    );
}

#[test]
fn default_range_has_1000_blocks() {
    assert_eq!(blocks((21000, 30999), 10).len(), 1000);
    assert_eq!(blocks((21000, 21999), 10).len(), 100);
}

#[test]
fn unaligned_range_aligns() {
    assert_eq!(blocks((21003, 21030), 10), vec![21010, 21020]);
}

struct Fake(Vec<u16>);
impl Prober for Fake {
    fn is_free(&self, port: u16) -> bool {
        !self.0.contains(&port)
    }
}

const RANGE: (u16, u16) = (21000, 21999);

fn setup() -> (Vec<u16>, usize) {
    let bs = blocks(RANGE, 10);
    let i = preferred_index(Path::new("/abs/hull"), "fix-login", bs.len());
    (bs, i)
}

#[test]
fn skips_taken_and_busy() {
    let (bs, i) = setup();
    let n = bs.len();
    let taken = [(bs[i], bs[i] + 9)];
    let prober = Fake(vec![bs[(i + 1) % n] + 3]);
    let got = allocate(
        Path::new("/abs/hull"),
        "fix-login",
        RANGE,
        10,
        &taken,
        &prober,
    );
    assert_eq!(got, Some(bs[(i + 2) % n]));
}

#[test]
fn prefers_index_when_free() {
    let (bs, i) = setup();
    let got = allocate(
        Path::new("/abs/hull"),
        "fix-login",
        RANGE,
        10,
        &[],
        &Fake(vec![]),
    );
    assert_eq!(got, Some(bs[i]));
}

#[test]
fn wraps_around() {
    // Pick a task whose preferred block is the last one, so the scan has to wrap past the end.
    let bs = blocks(RANGE, 10);
    let n = bs.len();
    let task = (0..10_000)
        .map(|k| format!("t{k}"))
        .find(|t| preferred_index(Path::new("/abs/hull"), t, n) == n - 1)
        .expect("a task hashing to the last block");
    let taken = [(bs[n - 1], bs[n - 1] + 9)];
    let got = allocate(
        Path::new("/abs/hull"),
        &task,
        RANGE,
        10,
        &taken,
        &Fake(vec![]),
    );
    assert_eq!(got, Some(bs[0]));
}

#[test]
fn none_when_full() {
    let taken = [(RANGE.0, RANGE.1)];
    assert_eq!(
        allocate(
            Path::new("/abs/hull"),
            "x",
            RANGE,
            10,
            &taken,
            &Fake(vec![])
        ),
        None
    );
    let all_busy = Fake((RANGE.0..=RANGE.1).collect());
    assert_eq!(
        allocate(Path::new("/abs/hull"), "x", RANGE, 10, &[], &all_busy),
        None
    );
}

#[test]
fn bind_prober_sees_listener() {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = l.local_addr().unwrap().port();
    assert!(!BindProber.is_free(port));
}

#[test]
fn bind_prober_sees_wildcard_v4() {
    let l = TcpListener::bind("0.0.0.0:0").unwrap();
    let port = l.local_addr().unwrap().port();
    assert!(!BindProber.is_free(port));
}

#[test]
fn bind_prober_sees_wildcard_v6() {
    let Ok(l) = TcpListener::bind("[::]:0") else {
        eprintln!("skipping: IPv6 bind unavailable");
        return;
    };
    let port = l.local_addr().unwrap().port();
    assert!(!BindProber.is_free(port));
}

#[test]
fn bind_prober_free_port() {
    // Another process may grab the port between drop and probe; any of 5 fresh ports reading
    // free proves the prober, while a prober that always says "busy" still fails.
    let free = (0..5).any(|_| {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        drop(l);
        BindProber.is_free(port)
    });
    assert!(free);
}
