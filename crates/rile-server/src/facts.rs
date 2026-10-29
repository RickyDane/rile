//! What the server tells Rile apps about itself.

use std::fs;
use std::path::Path;

use rile_remote::peer::{DeviceInfo, DeviceKind, LocalDevice};

use crate::config::Config;

#[derive(Clone, Debug, Default)]
pub struct Facts {
    pub host: String,
    pub model: String,
    pub os: String,
    pub memory: u64,
}

fn read_trimmed(path: &str) -> Option<String> {
    let s = fs::read_to_string(path).ok()?;
    let s = s.trim_matches(|c: char| c.is_whitespace() || c == '\0').to_string();
    // Firmware placeholders say nothing useful.
    let junk = ["", "To be filled by O.E.M.", "System Product Name", "Default string", "None", "Not Specified"];
    (!junk.contains(&s.as_str())).then_some(s)
}

/// "Raspberry Pi 4 Model B", "Dell Inc. PowerEdge R730", else the CPU.
fn model() -> String {
    if let Some(m) = read_trimmed("/proc/device-tree/model") {
        return m;
    }
    let vendor = read_trimmed("/sys/class/dmi/id/sys_vendor");
    if let Some(product) = read_trimmed("/sys/class/dmi/id/product_name") {
        return match vendor {
            Some(v) if !product.starts_with(&v) => format!("{v} {product}"),
            _ => product,
        };
    }
    let mut sys = sysinfo::System::new();
    sys.refresh_cpu_all();
    sys.cpus()
        .first()
        .map(|c| c.brand().trim().to_string())
        .filter(|b| !b.is_empty())
        .unwrap_or_else(|| "Server".into())
}

pub fn read() -> Facts {
    let mut sys = sysinfo::System::new();
    sys.refresh_memory();
    Facts {
        host: sysinfo::System::host_name().unwrap_or_else(|| "Rile Server".into()),
        model: model(),
        // "Ubuntu 24.04", "Debian GNU/Linux 12", "Alpine Linux 3.24.3"
        os: match (sysinfo::System::name(), sysinfo::System::os_version()) {
            (Some(n), Some(v)) if !n.contains(&v) => format!("{n} {v}"),
            (Some(n), _) => n,
            _ => sysinfo::System::long_os_version().unwrap_or_else(|| std::env::consts::OS.to_string()),
        },
        memory: sys.total_memory(),
    }
}

/// Used and total bytes of the disk holding `/`.
fn storage() -> (u64, u64) {
    let disks = sysinfo::Disks::new_with_refreshed_list();
    disks
        .iter()
        .filter(|d| Path::new("/").starts_with(d.mount_point()))
        .max_by_key(|d| d.mount_point().as_os_str().len())
        .map(|d| (d.total_space().saturating_sub(d.available_space()), d.total_space()))
        .unwrap_or((0, 0))
}

impl Facts {
    pub fn device_name(&self, config: &Config) -> String {
        Some(config.name.trim().to_string()).filter(|n| !n.is_empty()).unwrap_or_else(|| self.host.clone())
    }

    pub fn local_device(&self, config: &Config) -> LocalDevice {
        let shares = config.published();
        let (storage_used, storage_total) = storage();
        LocalDevice {
            info: DeviceInfo {
                id: String::new(),
                name: self.device_name(config),
                model: self.model.clone(),
                os: self.os.clone(),
                kind: DeviceKind::Server,
                memory: self.memory,
                storage_used,
                storage_total,
                shares: shares.iter().map(|s| s.name.clone()).collect(),
                writable: shares.iter().filter(|s| s.writable).map(|s| s.name.clone()).collect(),
                version: env!("CARGO_PKG_VERSION").to_string(),
            },
            shares,
        }
    }
}
