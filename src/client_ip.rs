use std::net::IpAddr;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Net {
    addr: IpAddr,
    prefix: u8,
}

impl Net {
    pub fn contains(&self, ip: IpAddr) -> bool {
        let ip = mapped_v4(ip);
        let addr = mapped_v4(self.addr);
        match (addr, ip) {
            (IpAddr::V4(net), IpAddr::V4(ip)) => {
                let mask = v4_mask(self.prefix);
                (u32::from(net) & mask) == (u32::from(ip) & mask)
            }
            (IpAddr::V6(net), IpAddr::V6(ip)) => {
                let mask = v6_mask(self.prefix);
                (u128::from(net) & mask) == (u128::from(ip) & mask)
            }
            _ => false,
        }
    }
}

pub fn default_trusted() -> Vec<Net> {
    parse_trusted(
        "127.0.0.0/8, ::1/128, 10.0.0.0/8, 172.16.0.0/12, 192.168.0.0/16, fc00::/7, fe80::/10",
    )
}

pub fn parse_trusted(raw: &str) -> Vec<Net> {
    raw.split(|c: char| c == ',' || c.is_whitespace())
        .filter_map(parse_net)
        .collect()
}

pub fn resolve_client_ip(
    peer: Option<IpAddr>,
    forwarded_for: Option<&str>,
    real_ip: Option<&str>,
    trusted: &[Net],
) -> String {
    let Some(peer) = peer else {
        return "local".into();
    };
    if !trusted.iter().any(|net| net.contains(peer)) {
        return peer.to_string();
    }
    if let Some(ip) = forwarded_for.and_then(rightmost_hop) {
        return ip.to_string();
    }
    if let Some(ip) = real_ip.and_then(parse_ip_token) {
        return ip.to_string();
    }
    peer.to_string()
}

fn parse_net(raw: &str) -> Option<Net> {
    let raw = raw.trim();
    if raw.is_empty() {
        return None;
    }
    let (addr_raw, prefix_raw) = match raw.split_once('/') {
        Some((addr, prefix)) => (addr, Some(prefix)),
        None => (raw, None),
    };
    let addr: IpAddr = addr_raw.trim().parse().ok()?;
    let prefix = match (prefix_raw, addr) {
        (None, IpAddr::V4(_)) => 32,
        (None, IpAddr::V6(_)) => 128,
        (Some(prefix), IpAddr::V4(_)) => prefix.trim().parse::<u8>().ok().filter(|n| *n <= 32)?,
        (Some(prefix), IpAddr::V6(_)) => prefix.trim().parse::<u8>().ok().filter(|n| *n <= 128)?,
    };
    Some(Net { addr, prefix })
}

fn rightmost_hop(header: &str) -> Option<IpAddr> {
    let hop = header
        .split(',')
        .map(str::trim)
        .rfind(|part| !part.is_empty())?;
    parse_ip_token(hop)
}

fn parse_ip_token(raw: &str) -> Option<IpAddr> {
    let raw = raw.trim().trim_matches('"');
    if raw.is_empty() {
        return None;
    }
    if let Ok(ip) = raw.parse::<IpAddr>() {
        return Some(ip);
    }
    if let Some(rest) = raw.strip_prefix('[') {
        let host = rest.split_once(']')?.0;
        return host.parse().ok();
    }
    let (host, port) = raw.rsplit_once(':')?;
    if !port.is_empty() && port.chars().all(|c| c.is_ascii_digit()) && !host.contains(':') {
        return host.parse().ok();
    }
    None
}

fn mapped_v4(ip: IpAddr) -> IpAddr {
    match ip {
        IpAddr::V6(ip) => ip
            .to_ipv4_mapped()
            .map(IpAddr::V4)
            .unwrap_or(IpAddr::V6(ip)),
        other => other,
    }
}

fn v4_mask(prefix: u8) -> u32 {
    if prefix == 0 {
        0
    } else {
        u32::MAX << (32 - u32::from(prefix))
    }
}

fn v6_mask(prefix: u8) -> u128 {
    if prefix == 0 {
        0
    } else {
        u128::MAX << (128 - u32::from(prefix))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_trusted_covers_loopback_private_and_docker() {
        let nets = default_trusted();
        for ip in ["127.0.0.1", "10.1.2.3", "172.17.0.2", "192.168.1.10", "::1"] {
            assert!(
                nets.iter().any(|net| net.contains(ip.parse().unwrap())),
                "{ip}"
            );
        }
        assert!(nets
            .iter()
            .any(|net| net.contains("fd00::1".parse().unwrap())));
        assert!(!nets
            .iter()
            .any(|net| net.contains("8.8.8.8".parse().unwrap())));
    }

    #[test]
    fn forwarded_for_is_used_only_behind_a_trusted_peer() {
        let trusted = parse_trusted("127.0.0.0/8, 10.0.0.0/8");
        let proxy = "127.0.0.1".parse().unwrap();
        assert_eq!(
            resolve_client_ip(Some(proxy), Some("1.2.3.4, 5.6.7.8"), None, &trusted),
            "5.6.7.8"
        );
        assert_eq!(
            resolve_client_ip(Some(proxy), Some("not-an-ip, 203.0.113.9"), None, &trusted),
            "203.0.113.9"
        );
        assert_eq!(
            resolve_client_ip(Some(proxy), None, Some("9.9.9.9"), &trusted),
            "9.9.9.9"
        );
        assert_eq!(
            resolve_client_ip(Some(proxy), Some("garbage"), Some("9.9.9.9"), &trusted),
            "9.9.9.9"
        );
        assert_eq!(
            resolve_client_ip(Some(proxy), None, None, &trusted),
            "127.0.0.1"
        );
        let client = "8.8.8.8".parse().unwrap();
        assert_eq!(
            resolve_client_ip(
                Some(client),
                Some("1.2.3.4, 5.6.7.8"),
                Some("9.9.9.9"),
                &trusted
            ),
            "8.8.8.8"
        );
        assert_eq!(
            resolve_client_ip(None, Some("1.2.3.4"), None, &trusted),
            "local"
        );
    }

    #[test]
    fn invalid_proxy_entries_are_skipped() {
        let nets = parse_trusted("192.0.2.1, not-a-cidr, 2001:db8::/32");
        assert!(nets
            .iter()
            .any(|net| net.contains("192.0.2.1".parse().unwrap())));
        assert!(nets
            .iter()
            .any(|net| net.contains("2001:db8::5".parse().unwrap())));
        assert!(!nets
            .iter()
            .any(|net| net.contains("192.0.2.2".parse().unwrap())));
    }
}
