//! Finding other Rile devices on the local network (DNS-SD `_rile._tcp`) and
//! announcing this one.

use std::collections::BTreeMap;
use std::net::IpAddr;

use anyhow::Result;
use mdns_sd::{Receiver, ResolvedService, ServiceDaemon, ServiceEvent, ServiceInfo};

use super::wire::{DeviceInfo, DeviceKind};

pub const SERVICE_TYPE: &str = "_rile._tcp.local.";

/// Another Rile device seen on the network.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NearbyPeer {
    pub id: String,
    pub name: String,
    pub model: String,
    pub os: String,
    pub kind: DeviceKind,
    /// Installed RAM in bytes; 0 when not advertised.
    pub memory: u64,
    /// Address to connect to (an IPv4 address when advertised, else the host name).
    pub host: String,
    pub port: u16,
}

/// Browses for other devices and advertises this one. Dropping it stops both.
pub struct PeerDiscovery {
    daemon: ServiceDaemon,
    browser: Receiver<ServiceEvent>,
    own_id: String,
    registered: Option<String>,
    peers: BTreeMap<String, NearbyPeer>,
}

fn truncate(s: &str, max: usize) -> String {
    let mut out = String::new();
    for c in s.chars() {
        if out.len() + c.len_utf8() > max {
            break;
        }
        out.push(c);
    }
    out
}

fn kind_str(kind: DeviceKind) -> &'static str {
    match kind {
        DeviceKind::Laptop => "laptop",
        DeviceKind::Desktop => "desktop",
        DeviceKind::Server => "server",
    }
}

fn parse_kind(s: &str) -> DeviceKind {
    match s {
        "laptop" => DeviceKind::Laptop,
        "server" => DeviceKind::Server,
        _ => DeviceKind::Desktop,
    }
}

impl PeerDiscovery {
    pub fn new(own_id: &str) -> Result<Self> {
        let daemon = ServiceDaemon::new()?;
        let browser = daemon.browse(SERVICE_TYPE)?;
        Ok(Self { daemon, browser, own_id: own_id.to_string(), registered: None, peers: BTreeMap::new() })
    }

    /// Announces (or re-announces, after a change) this device on `port`.
    pub fn advertise(&mut self, info: &DeviceInfo, port: u16) -> Result<()> {
        if let Some(old) = self.registered.take() {
            let _ = self.daemon.unregister(&old);
        }
        // The instance name must be unique on the network; the device name
        // travels in the TXT record.
        let instance = format!("rile-{}", &self.own_id[..self.own_id.len().min(16)]);
        let host = format!("{instance}.local.");
        let props = [
            ("id", self.own_id.clone()),
            ("v", super::wire::PROTOCOL_VERSION.to_string()),
            ("name", truncate(&info.name, 120)),
            ("model", truncate(&info.model, 80)),
            ("os", truncate(&info.os, 40)),
            ("kind", kind_str(info.kind).to_string()),
            ("mem", info.memory.to_string()),
        ];
        let props: Vec<(&str, &str)> = props.iter().map(|(k, v)| (*k, v.as_str())).collect();
        let service = ServiceInfo::new(SERVICE_TYPE, &instance, &host, "", port, &props[..])?.enable_addr_auto();
        let fullname = service.get_fullname().to_string();
        self.daemon.register(service)?;
        self.registered = Some(fullname);
        Ok(())
    }

    /// Drains queued updates without blocking.
    pub fn poll(&mut self) -> Vec<NearbyPeer> {
        while let Ok(event) = self.browser.try_recv() {
            apply_event(&mut self.peers, &self.own_id, event);
        }
        let mut peers: Vec<_> = self.peers.values().cloned().collect();
        peers.sort_by_key(|p| (p.name.to_lowercase(), p.id.clone()));
        peers.dedup_by(|a, b| a.id == b.id);
        peers
    }
}

fn best_address(info: &ResolvedService) -> Option<IpAddr> {
    let mut v4: Vec<_> = info.get_addresses_v4().into_iter().collect();
    // Prefer routable LAN addresses over link-local ones.
    v4.sort_by_key(|a| (a.is_link_local(), a.is_loopback(), *a));
    v4.first().map(|a| IpAddr::V4(*a))
}

fn apply_event(peers: &mut BTreeMap<String, NearbyPeer>, own_id: &str, event: ServiceEvent) {
    match event {
        ServiceEvent::ServiceResolved(info) => {
            let Some(id) = info.get_property_val_str("id").map(str::to_string) else { return };
            if id == own_id || id.len() != 64 {
                return;
            }
            let txt = |k: &str| info.get_property_val_str(k).unwrap_or_default().to_string();
            let host = best_address(&info)
                .map(|a| a.to_string())
                .unwrap_or_else(|| info.get_hostname().trim_end_matches('.').to_string());
            let name = Some(txt("name")).filter(|n| !n.is_empty()).unwrap_or_else(|| super::identity::short_id(&id));
            peers.insert(
                info.get_fullname().to_string(),
                NearbyPeer {
                    id,
                    name,
                    model: txt("model"),
                    os: txt("os"),
                    kind: parse_kind(&txt("kind")),
                    memory: txt("mem").parse().unwrap_or(0),
                    host,
                    port: info.get_port(),
                },
            );
        }
        ServiceEvent::ServiceRemoved(_, fullname) => {
            peers.remove(&fullname);
        }
        _ => {}
    }
}

impl Drop for PeerDiscovery {
    fn drop(&mut self) {
        if let Some(name) = self.registered.take() {
            let _ = self.daemon.unregister(&name);
        }
        let _ = self.daemon.shutdown();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn resolved(id: &str, name: &str) -> ServiceEvent {
        let props = [("id", id), ("name", name), ("model", "Mac Studio"), ("kind", "laptop")];
        ServiceEvent::ServiceResolved(Box::new(
            ServiceInfo::new(SERVICE_TYPE, &format!("rile-{}", &id[..16]), "studio.local.", "192.0.2.7", 47820, &props[..])
                .unwrap()
                .as_resolved_service(),
        ))
    }

    #[test]
    fn parses_peers_and_skips_itself() {
        let own = "a".repeat(64);
        let other = "b".repeat(64);
        let mut peers = BTreeMap::new();
        apply_event(&mut peers, &own, resolved(&own, "Me"));
        assert!(peers.is_empty());
        apply_event(&mut peers, &own, resolved(&other, "Studio"));
        let p = peers.values().next().unwrap();
        assert_eq!(
            (p.name.as_str(), p.model.as_str(), p.kind, p.host.as_str(), p.port),
            ("Studio", "Mac Studio", DeviceKind::Laptop, "192.0.2.7", 47820)
        );
        let full = peers.keys().next().unwrap().clone();
        apply_event(&mut peers, &own, ServiceEvent::ServiceRemoved(SERVICE_TYPE.into(), full));
        assert!(peers.is_empty());
    }
}
