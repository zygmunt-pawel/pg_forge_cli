use crate::error::{PgForgeError, Result};
use std::collections::HashSet;
use std::net::TcpListener;

/// Abstraction over "can I bind to this port right now?" so the allocator
/// algorithm stays unit-testable without touching the OS.
pub trait IsBindable {
    fn is_bindable(&self, port: u16) -> bool;
}

/// Real probe: tries to bind to 127.0.0.1:port and immediately drops the
/// listener. If the bind succeeds, the port is free at this instant.
pub struct TcpProbe;

impl IsBindable for TcpProbe {
    fn is_bindable(&self, port: u16) -> bool {
        TcpListener::bind(("127.0.0.1", port)).is_ok()
    }
}

/// Find the first port in `[start, end)` that is neither in `taken` nor
/// currently in use according to `probe`.
pub fn allocate_port<P: IsBindable>(
    start: u16,
    end: u16,
    taken: &HashSet<u16>,
    probe: &P,
) -> Result<u16> {
    for p in start..end {
        if taken.contains(&p) {
            continue;
        }
        if probe.is_bindable(p) {
            return Ok(p);
        }
    }
    Err(PgForgeError::NoFreePort { start, end })
}
