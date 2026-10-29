//! Nearby Devices: Rile ↔ Rile on the local network.
//!
//! Every installation has a self-signed certificate; its SHA-256 is the
//! device id. Devices find each other with DNS-SD (`_rile._tcp`), talk over
//! mutual TLS 1.3, and pair once by confirming the same 6-digit code on both
//! screens (derived from the TLS session, so a relay in the middle would show
//! different codes). Paired devices can browse each other's shared folders
//! (read-only, as [`crate::Protocol::Peer`] connections) and send each other
//! files, which the receiving user accepts or declines.

mod client;
mod discovery;
mod identity;
mod server;
mod tls;
mod wire;

pub(crate) use client::PeerBackend;
pub use client::{IdentityMismatch, Pairing, describe, probe, send, set_local_device};
pub use discovery::{NearbyPeer, PeerDiscovery, SERVICE_TYPE};
pub use identity::{PeerStore, TrustedPeer, fingerprint, short_id};
pub use server::{LocalDevice, PROMPT_TIMEOUT, PeerEvent, PeerServer, Reply, Share, Transfer};
pub use tls::format_code;
pub use wire::{DeviceInfo, DeviceKind, ErrorKind, OfferItem, PeerError, is_not_paired};

/// Port the server tries first.
pub const DEFAULT_PORT: u16 = 47820;
const CONN_PREFIX: &str = "peer:";

/// Connection id used for browsing the device `peer_id`.
pub fn conn_id(peer_id: &str) -> String {
    format!("{CONN_PREFIX}{peer_id}")
}

/// The device id of a connection made with [`conn_id`].
pub fn peer_of_conn(conn: &str) -> Option<&str> {
    conn.strip_prefix(CONN_PREFIX)
}

/// A [`crate::ConnectionInfo`] for browsing `peer` at its current address.
pub fn connection_info(id: &str, name: &str, host: &str, port: u16) -> crate::ConnectionInfo {
    crate::ConnectionInfo {
        id: conn_id(id),
        name: name.to_string(),
        protocol: crate::Protocol::Peer,
        host: host.to_string(),
        port,
        username: id.to_string(),
        auth: crate::Auth::Anonymous,
        initial_path: None,
    }
}
