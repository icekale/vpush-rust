use std::fmt;
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

impl fmt::Display for Net {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}/{}", self.addr, self.prefix)
    }
}

pub fn default_trusted() -> Vec<Net> {
    parse_trusted("127.0.0.0/8, ::1/128")
}

pub fn parse_trusted(raw: &str) -> Vec<Net> {
    raw.split(|c: char| c == ',' || c.is_whitespace())
        .filter_map(parse_net)
        .collect()
}

pub fn resolve_client_ip(
    peer: Option<IpAddr>,
    forwarded_for: &[&str],
    real_ip: Option<&str>,
    trust_real_ip: bool,
    trusted: &[Net],
) -> String {
    let Some(peer) = peer else {
        return "local".into();
    };
    if !trusted.iter().any(|net| net.contains(peer)) {
        return peer.to_string();
    }
    if let Some(header) = forwarded_for.last() {
        let hops = header_hops(header);
        if let Some(ip) = client_from_hops(&hops, trusted) {
            return ip.to_string();
        }
    }
    if trust_real_ip {
        if let Some(ip) = real_ip.and_then(parse_ip_token) {
            return ip.to_string();
        }
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

fn header_hops(header: &str) -> Vec<IpAddr> {
    header.split(',').filter_map(parse_ip_token).collect()
}

fn client_from_hops(hops: &[IpAddr], trusted: &[Net]) -> Option<IpAddr> {
    let mut chosen = None;
    for hop in hops.iter().rev() {
        chosen = Some(*hop);
        if !trusted.iter().any(|net| net.contains(*hop)) {
            break;
        }
    }
    chosen
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
    fn default_trusted_is_loopback_only() {
        let nets = default_trusted();
        for ip in ["127.0.0.1", "::1", "::ffff:127.0.0.2"] {
            assert!(
                nets.iter().any(|net| net.contains(ip.parse().unwrap())),
                "{ip}"
            );
        }
        for ip in [
            "10.1.2.3",
            "172.17.0.2",
            "192.168.1.10",
            "fd00::1",
            "8.8.8.8",
        ] {
            assert!(
                !nets.iter().any(|net| net.contains(ip.parse().unwrap())),
                "{ip}"
            );
        }
        let docker = parse_trusted("172.16.0.0/12");
        assert!(docker
            .iter()
            .any(|net| net.contains("172.17.0.2".parse().unwrap())));
    }

    #[test]
    fn forwarded_for_walks_the_last_header_from_the_right() {
        let trusted = parse_trusted("127.0.0.0/8, 10.0.0.0/8");
        let proxy = "127.0.0.1".parse().unwrap();
        let chain = ["9.9.9.9, 8.8.4.4", "1.2.3.4, 10.1.1.1"];
        assert_eq!(
            resolve_client_ip(Some(proxy), &chain, None, false, &trusted),
            "1.2.3.4"
        );
        let untrusted_right = ["203.0.113.9, 198.51.100.8"];
        assert_eq!(
            resolve_client_ip(Some(proxy), &untrusted_right, None, false, &trusted),
            "198.51.100.8"
        );
        let all_trusted = ["10.2.2.2, 10.1.1.1"];
        assert_eq!(
            resolve_client_ip(Some(proxy), &all_trusted, None, false, &trusted),
            "10.2.2.2"
        );
        assert_eq!(
            resolve_client_ip(Some(proxy), &["garbage"], Some("9.9.9.9"), false, &trusted),
            "127.0.0.1"
        );
        assert_eq!(
            resolve_client_ip(Some(proxy), &["garbage"], Some("9.9.9.9"), true, &trusted),
            "9.9.9.9"
        );
        assert_eq!(
            resolve_client_ip(Some(proxy), &[] as &[&str], None, false, &trusted),
            "127.0.0.1"
        );
        let client = "8.8.8.8".parse().unwrap();
        assert_eq!(
            resolve_client_ip(Some(client), &chain, Some("9.9.9.9"), true, &trusted),
            "8.8.8.8"
        );
        let loopback_only = default_trusted();
        let docker = "172.17.0.2".parse().unwrap();
        assert_eq!(
            resolve_client_ip(Some(docker), &chain, Some("9.9.9.9"), true, &loopback_only),
            "172.17.0.2"
        );
        assert_eq!(
            resolve_client_ip(None, &["1.2.3.4"], None, false, &trusted),
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
