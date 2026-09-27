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
    if host.parse::<IpAddr>().is_ok_and(|ip| is_private(&ip)) {
        return Err("URL 不能指向内网".into());
    }
    Ok(())
}

pub fn public_resolver(netloc: &str) -> io::Result<Vec<SocketAddr>> {
    let addrs: Vec<_> = netloc.to_socket_addrs()?.collect();
    if addrs.is_empty() || addrs.iter().any(|addr| is_private(&addr.ip())) {
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

fn is_private(ip: &IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => {
            ip.is_private()
                || ip.is_loopback()
                || ip.is_link_local()
                || ip.is_unspecified()
                || ip.is_broadcast()
                || ip.is_documentation()
        }
        IpAddr::V6(ip) => {
            ip.is_loopback()
                || ip.is_unspecified()
                || ip.is_unique_local()
                || ip.is_unicast_link_local()
                || ip
                    .to_ipv4_mapped()
                    .is_some_and(|ip| is_private(&IpAddr::V4(ip)))
        }
    }
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
        ] {
            assert!(
                validate_url(&format!("https://{host}/"), "https").is_err(),
                "{host}"
            );
        }
        assert!(validate_url("https://example.com/", "https").is_ok());
    }

    #[test]
    fn rejects_private_resolver_results() {
        assert!(public_resolver("127.0.0.1:443").is_err());
        assert!(public_resolver("[::1]:443").is_err());
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
