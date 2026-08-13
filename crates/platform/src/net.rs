use std::net::IpAddr;
use std::sync::mpsc::{self, Receiver};
use std::thread;
use std::time::Duration;

/// 本机可用于 Hub-assisted LAN Direct 的候选地址。不含账号信息。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LanCandidate {
    pub ip: IpAddr,
    pub port: u16,
}

impl LanCandidate {
    pub fn endpoint(&self) -> String {
        match self.ip {
            IpAddr::V4(ip) => format!("{ip}:{}", self.port),
            IpAddr::V6(ip) => format!("[{ip}]:{}", self.port),
        }
    }
}

pub fn local_candidates(port: u16) -> Vec<LanCandidate> {
    if_addrs::get_if_addrs()
        .unwrap_or_default()
        .into_iter()
        .filter(|iface| !iface.is_loopback())
        .map(|iface| LanCandidate { ip: iface.ip(), port })
        .collect()
}

/// 网络变化后调用方应清理 Direct、重建 candidate、重启 mDNS。
pub fn spawn_change_watch(interval: Duration) -> Receiver<()> {
    let (tx, rx) = mpsc::channel();
    thread::Builder::new()
        .name("asterism-net-watch".into())
        .spawn(move || {
            let mut last = fingerprint();
            loop {
                thread::sleep(interval);
                let now = fingerprint();
                if now != last {
                    last = now;
                    if tx.send(()).is_err() {
                        break;
                    }
                }
            }
        })
        .ok();
    rx
}

fn fingerprint() -> String {
    let mut parts: Vec<String> = if_addrs::get_if_addrs()
        .unwrap_or_default()
        .into_iter()
        .map(|i| format!("{}={}", i.name, i.ip()))
        .collect();
    parts.sort();
    parts.join("|")
}
