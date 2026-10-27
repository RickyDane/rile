//! Local-network DNS-SD discovery. Only advertised services are discovered;
//! servers without Bonjour/mDNS can always be entered manually.

use std::collections::BTreeMap;

use anyhow::Result;
use mdns_sd::{Receiver, ServiceDaemon, ServiceEvent};

use crate::Protocol;

const SERVICES: [(&str, Protocol); 3] = [
    ("_ftp._tcp.local.", Protocol::Ftp),
    ("_smb._tcp.local.", Protocol::Smb),
    ("_sftp-ssh._tcp.local.", Protocol::Sftp),
];

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiscoveredServer {
    pub id: String,
    pub name: String,
    pub protocol: Protocol,
    pub host: String,
    pub port: u16,
}

/// Owns the browser lifetime. Dropping it stops discovery and its worker thread.
pub struct Discovery {
    daemon: ServiceDaemon,
    browsers: Vec<(Protocol, Receiver<ServiceEvent>)>,
    servers: BTreeMap<String, DiscoveredServer>,
}

impl Discovery {
    pub fn new() -> Result<Self> {
        let daemon = ServiceDaemon::new()?;
        let mut discovery = Self {
            daemon,
            browsers: Vec::new(),
            servers: BTreeMap::new(),
        };
        for (service, protocol) in SERVICES {
            discovery
                .browsers
                .push((protocol, discovery.daemon.browse(service)?));
        }
        Ok(discovery)
    }

    /// Drain queued updates without blocking the UI or doing network I/O there.
    pub fn poll(&mut self) -> Vec<DiscoveredServer> {
        for (protocol, events) in &self.browsers {
            while let Ok(event) = events.try_recv() {
                apply_event(&mut self.servers, *protocol, event);
            }
        }
        let mut servers: Vec<_> = self.servers.values().cloned().collect();
        servers.sort_by_key(|s| (s.name.to_lowercase(), s.protocol.label(), s.id.clone()));
        servers
    }
}

fn apply_event(
    servers: &mut BTreeMap<String, DiscoveredServer>,
    protocol: Protocol,
    event: ServiceEvent,
) {
    match event {
        ServiceEvent::ServiceResolved(info) => {
            let id = info.get_fullname().to_string();
            let Some((suffix, _)) = SERVICES.iter().find(|(_, p)| *p == protocol) else {
                return;
            };
            let name = id
                .strip_suffix(&format!(".{suffix}"))
                .unwrap_or(&id)
                .to_string();
            servers.insert(
                id.clone(),
                DiscoveredServer {
                    id,
                    name,
                    protocol,
                    host: info.get_hostname().trim_end_matches('.').to_string(),
                    port: info.get_port(),
                },
            );
        }
        ServiceEvent::ServiceRemoved(_, id) => {
            servers.remove(&id);
        }
        _ => {}
    }
}

impl Drop for Discovery {
    fn drop(&mut self) {
        let _ = self.daemon.shutdown();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mdns_sd::ServiceInfo;

    fn resolved(service: &str, port: u16) -> ServiceEvent {
        ServiceEvent::ServiceResolved(Box::new(
            ServiceInfo::new(service, "Test NAS", "nas.local.", "192.0.2.1", port, None)
                .unwrap()
                .as_resolved_service(),
        ))
    }

    #[test]
    fn resolves_updates_and_removes_advertisements() {
        let mut servers = BTreeMap::new();
        apply_event(
            &mut servers,
            Protocol::Ftp,
            resolved("_ftp._tcp.local.", 2121),
        );
        apply_event(
            &mut servers,
            Protocol::Smb,
            resolved("_smb._tcp.local.", 445),
        );
        assert_eq!(servers.len(), 2);
        let ftp = &servers["Test NAS._ftp._tcp.local."];
        assert_eq!(
            (&*ftp.name, &*ftp.host, ftp.port, ftp.protocol),
            ("Test NAS", "nas.local", 2121, Protocol::Ftp)
        );
        apply_event(
            &mut servers,
            Protocol::Ftp,
            resolved("_ftp._tcp.local.", 21),
        );
        assert_eq!(servers.len(), 2);
        assert_eq!(servers["Test NAS._ftp._tcp.local."].port, 21);
        apply_event(
            &mut servers,
            Protocol::Ftp,
            ServiceEvent::ServiceRemoved(
                "_ftp._tcp.local.".into(),
                "Test NAS._ftp._tcp.local.".into(),
            ),
        );
        assert_eq!(servers.len(), 1);
        assert_eq!(servers.values().next().unwrap().protocol, Protocol::Smb);
    }

    #[test]
    #[ignore = "requires local-network multicast access"]
    fn discovers_live_ftp_and_smb_advertisements() {
        use std::time::{Duration, Instant};

        let mut browser = Discovery::new().unwrap();
        let publisher = Discovery::new().unwrap();
        let name = format!("Rile-Test-{}", crate::ConnectionInfo::new_id());
        let host = format!("rile-test-{}.local.", std::process::id());
        for (service, protocol) in SERVICES.into_iter().take(2) {
            publisher
                .daemon
                .register(
                    ServiceInfo::new(service, &name, &host, "", protocol.default_port(), None)
                        .unwrap()
                        .enable_addr_auto(),
                )
                .unwrap();
        }
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let servers = browser.poll();
            let found: Vec<_> = servers
                .iter()
                .filter(|server| server.name == name)
                .collect();
            if found.len() == 2 {
                assert!(
                    found
                        .iter()
                        .any(|server| server.protocol == Protocol::Ftp && server.port == 21)
                );
                assert!(
                    found
                        .iter()
                        .any(|server| server.protocol == Protocol::Smb && server.port == 445)
                );
                break;
            }
            assert!(
                Instant::now() < deadline,
                "Timed out discovering test advertisements: {servers:?}"
            );
            std::thread::sleep(Duration::from_millis(100));
        }
        for (service, _) in SERVICES.into_iter().take(2) {
            publisher
                .daemon
                .unregister(&format!("{name}.{service}"))
                .unwrap();
        }
        let deadline = Instant::now() + Duration::from_secs(10);
        while browser.poll().iter().any(|server| server.name == name) {
            assert!(
                Instant::now() < deadline,
                "Removed services remained in discovery"
            );
            std::thread::sleep(Duration::from_millis(100));
        }
    }
}
