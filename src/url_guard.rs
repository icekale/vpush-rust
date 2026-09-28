use std::io;
use std::net::{IpAddr, SocketAddr, ToSocketAddrs};

pub fn validate_url(raw: &str, scheme: &str) -> Result<(), String> {
    let prefix = format!("{scheme}://");
    let Some(rest) = raw.strip_prefix(&prefix) else {
        return Err(format!("只接受 {scheme} URL"));
    };
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    let host = authority_host(authority)?;
    if host.eq_ignore_ascii_case("localhost")
        || host.ends_with(".local")
        || host.ends_with(".localhost")
    {
        return Err("URL 不能指向本机".into());
    }
    if host.parse::<IpAddr>().is_ok_and(ip_blocked) {
        return Err("URL 不能指向内网".into());
    }
    Ok(())
}

pub fn public_resolver(netloc: &str) -> io::Result<Vec<SocketAddr>> {
    let addrs: Vec<_> = netloc.to_socket_addrs()?.collect();
    if addrs.is_empty() || addrs.iter().any(|addr| ip_blocked(addr.ip())) {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "private address",
        ));
    }
    Ok(addrs)
}

fn authority_host(authority: &str) -> Result<String, String> {
    if authority.is_empty()
        || authority.contains('@')
        || authority
            .chars()
            .any(|c| c.is_control() || c.is_whitespace())
    {
        return Err("URL 地址无效".into());
    }
    let host = if let Some(rest) = authority.strip_prefix('[') {
        let end = rest.find(']').ok_or_else(|| "URL 地址无效".to_string())?;
        if !rest[end + 1..].is_empty() && !rest[end + 1..].starts_with(':') {
            return Err("URL 地址无效".into());
        }
        &rest[..end]
    } else {
        match authority.rsplit_once(':') {
            Some((host, port)) if !port.is_empty() => {
                if !port.chars().all(|c| c.is_ascii_digit()) || port.parse::<u16>().is_err() {
                    return Err("URL 地址无效".into());
                }
                host
            }
            Some((_host, _)) => return Err("URL 地址无效".into()),
            None => authority,
        }
    };
    if host.is_empty() || host.contains(':') && host.parse::<IpAddr>().is_err() {
        return Err("URL 地址无效".into());
    }
    Ok(host.trim_matches(['[', ']']).to_ascii_lowercase())
}

fn nat64_embedded(ip: &std::net::Ipv6Addr) -> Option<std::net::Ipv4Addr> {
    let seg = ip.segments();
    // ponytail: only the well-known 64:ff9b::/96 prefix; other NAT64 prefixes need an explicit allowlist.
    if seg[0] != 0x0064 || seg[1] != 0xff9b || seg[2..6].iter().any(|part| *part != 0) {
        return None;
    }
    Some(std::net::Ipv4Addr::new(
        (seg[6] >> 8) as u8,
        seg[6] as u8,
        (seg[7] >> 8) as u8,
        seg[7] as u8,
    ))
}

pub fn ip_blocked(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => ipv4_blocked(ip),
        IpAddr::V6(ip) => ipv6_blocked(ip),
    }
}

fn ipv4_blocked(ip: std::net::Ipv4Addr) -> bool {
    let [a, b, c, _] = ip.octets();
    ip.is_loopback()
        || ip.is_private()
        || ip.is_link_local()
        || ip.is_broadcast()
        || ip.is_multicast()
        || ip.is_unspecified()
        || ip.is_documentation()
        || a == 0
        || (a == 100 && (64..128).contains(&b))
        || (a == 192 && b == 0 && c == 0)
        || (a == 198 && (b == 18 || b == 19))
        || a >= 224
}

fn ipv6_blocked(ip: std::net::Ipv6Addr) -> bool {
    if ip.is_loopback()
        || ip.is_unspecified()
        || ip.is_unique_local()
        || ip.is_unicast_link_local()
        || ip.is_multicast()
    {
        return true;
    }
    let seg = ip.segments();
    // fec0::/10 site-local, 2001:db8::/32 documentation, 100::/64 discard-only.
    if seg[0] & 0xffc0 == 0xfec0
        || (seg[0] == 0x2001 && seg[1] == 0x0db8)
        || (seg[0] == 0x0100 && seg[1] == 0 && seg[2] == 0 && seg[3] == 0)
        || seg[0] == 0x2002
        || (seg[0] == 0x2001 && seg[1] == 0)
    {
        return true;
    }
    ip.to_ipv4().is_some_and(ipv4_blocked) || nat64_embedded(&ip).is_some_and(ipv4_blocked)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_private_and_special_addresses() {
        for host in [
            "127.0.0.1",
            "10.1.2.3",
            "169.254.169.254",
            "[::1]",
            "[FD00::1]",
            "[FE80::1]",
            "[::ffff:127.0.0.1]",
            "[64:ff9b::a00:1]",
            "[64:ff9b::7f00:1]",
            "[64:ff9b::a9fe:a9fe]",
            "100.64.0.1",
            "0.1.2.3",
            "198.18.0.1",
            "240.0.0.1",
            "224.0.0.1",
            "192.0.0.8",
            "[ff02::1]",
            "[2002:7f00:1::]",
            "[2001:0::1]",
            "[2001:db8::1]",
            "[100::1]",
            "[fec0::1]",
        ] {
            assert!(
                validate_url(&format!("https://{host}/"), "https").is_err(),
                "{host}"
            );
        }
        assert!(validate_url("https://example.com/", "https").is_ok());
        assert!(validate_url("https://1.1.1.1/", "https").is_ok());
        assert!(validate_url("https://8.8.8.8/", "https").is_ok());
        assert!(validate_url("https://[64:ff9b::808:808]/", "https").is_ok());
        assert!(validate_url("https://[2606:4700:4700::1111]/", "https").is_ok());
    }

    #[test]
    fn rejects_private_resolver_results() {
        assert!(public_resolver("127.0.0.1:443").is_err());
        assert!(public_resolver("[::1]:443").is_err());
        assert!(public_resolver("[64:ff9b::a00:1]:443").is_err());
        assert!(public_resolver("100.64.1.1:443").is_err());
        assert!(public_resolver("[2002:a00:1::]:443").is_err());
        assert!(public_resolver("[ff02::1]:443").is_err());
    }

    #[test]
    fn rejects_credentials_and_invalid_authority() {
        for raw in [
            "https://user@host.example/",
            "https://host.example:bad/",
            "https://host.example\\@127.0.0.1/",
            "https://[::1",
        ] {
            assert!(validate_url(raw, "https").is_err(), "{raw}");
        }
    }
}
