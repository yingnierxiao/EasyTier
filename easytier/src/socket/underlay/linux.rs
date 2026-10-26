/// 只接受可转发的主表默认路由，忽略 VPN 的半网路由、拒绝路由和 IPv6 源特定路由。
fn default_routes(text: &str, ipv6: bool) -> Vec<(&str, u32)> {
    text.lines()
        .filter_map(|line| {
            let mut fields = line.split_ascii_whitespace();
            let (name, metric, flags) = if ipv6 {
                let destination = fields.next()?;
                let prefix = fields.next()?;
                let source = fields.next()?;
                let source_prefix = fields.next()?;
                let _gateway = fields.next()?;
                let metric = u32::from_str_radix(fields.next()?, 16).ok()?;
                let _ref_count = fields.next()?;
                let _use_count = fields.next()?;
                let flags = u32::from_str_radix(fields.next()?, 16).ok()?;
                let name = fields.next()?;
                if destination != "00000000000000000000000000000000"
                    || prefix != "00"
                    || source != "00000000000000000000000000000000"
                    || source_prefix != "00"
                {
                    return None;
                }
                (name, metric, flags)
            } else {
                let name = fields.next()?;
                let destination = fields.next()?;
                let _gateway = fields.next()?;
                let flags = u32::from_str_radix(fields.next()?, 16).ok()?;
                let _ref_count = fields.next()?;
                let _use_count = fields.next()?;
                let metric = fields.next()?.parse().ok()?;
                let mask = fields.next()?;
                if destination != "00000000" || mask != "00000000" {
                    return None;
                }
                (name, metric, flags)
            };
            // Linux RTF_UP / RTF_REJECT，与 IPv4、IPv6 的内核导出格式一致。
            (flags & 1 != 0 && flags & 0x200 == 0).then_some((name, metric))
        })
        .collect()
}

fn select_route<'a>(
    routes: &[(&'a str, u32)],
    mut is_underlay: impl FnMut(&str) -> bool,
) -> Option<&'a str> {
    routes
        .iter()
        .filter(|(name, _)| is_underlay(name))
        .min_by_key(|(name, metric)| (*metric, *name))
        .map(|(name, _)| *name)
}

#[cfg(target_os = "linux")]
pub(super) fn default_interface(ipv6: bool) -> std::io::Result<super::unix::Interface> {
    let text = std::fs::read_to_string(if ipv6 {
        "/proc/thread-self/net/ipv6_route"
    } else {
        "/proc/thread-self/net/route"
    })?;
    let interfaces = underlay_interfaces()?;
    let routes = default_routes(&text, ipv6);
    let name = select_route(&routes, |name| {
        interfaces.iter().any(|(candidate, _)| candidate == name)
    })
    .ok_or_else(|| super::unix::no_default_route(ipv6))?;
    let index = interfaces
        .iter()
        .find(|(candidate, _)| candidate == name)
        .unwrap()
        .1;
    Ok(super::unix::Interface {
        name: name.to_owned(),
        index,
    })
}

/// 使用当前网络命名空间的 RTM_GETLINK，不能用可能挂载在其他空间的 sysfs 判断 TAP。
#[cfg(target_os = "linux")]
fn underlay_interfaces() -> std::io::Result<Vec<(String, u32)>> {
    use nix::libc;
    use std::{
        io::{self, Read, Write},
        os::fd::{AsRawFd, FromRawFd},
    };

    let fd = unsafe {
        libc::socket(
            libc::AF_NETLINK,
            libc::SOCK_RAW | libc::SOCK_CLOEXEC,
            libc::NETLINK_ROUTE,
        )
    };
    if fd == -1 {
        return Err(io::Error::last_os_error());
    }
    let mut socket = unsafe { std::fs::File::from_raw_fd(fd) };
    let mut kernel: libc::sockaddr_nl = unsafe { std::mem::zeroed() };
    kernel.nl_family = libc::AF_NETLINK as _;
    if unsafe {
        libc::connect(
            socket.as_raw_fd(),
            (&kernel as *const libc::sockaddr_nl).cast(),
            std::mem::size_of_val(&kernel) as _,
        )
    } == -1
    {
        return Err(io::Error::last_os_error());
    }
    let timeout = libc::timeval {
        tv_sec: 1,
        tv_usec: 0,
    };
    if unsafe {
        libc::setsockopt(
            socket.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_RCVTIMEO,
            (&timeout as *const libc::timeval).cast(),
            std::mem::size_of_val(&timeout) as _,
        )
    } == -1
    {
        return Err(io::Error::last_os_error());
    }
    // nlmsghdr + ifinfomsg；RTM_GETLINK=18，NLM_F_REQUEST | NLM_F_DUMP。
    let mut request = [0u8; 32];
    request[..4].copy_from_slice(&32u32.to_ne_bytes());
    request[4..6].copy_from_slice(&18u16.to_ne_bytes());
    request[6..8].copy_from_slice(&0x301u16.to_ne_bytes());
    request[8..12].copy_from_slice(&1u32.to_ne_bytes());
    socket.write_all(&request)?;
    let mut interfaces = Vec::new();
    let mut buffer = vec![0; 65536];
    loop {
        let length = socket.read(&mut buffer)?;
        if length == 0 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "netlink route dump closed",
            ));
        }
        let mut messages = &buffer[..length];
        while !messages.is_empty() {
            if messages.len() < 16 {
                return Err(invalid_link());
            }
            let length = u32::from_ne_bytes(messages[..4].try_into().unwrap()) as usize;
            if !(16..=messages.len()).contains(&length) {
                return Err(invalid_link());
            }
            let message = &messages[..length];
            let flags = u16::from_ne_bytes(message[6..8].try_into().unwrap());
            if flags & 0x10 != 0 {
                return Err(io::Error::new(
                    io::ErrorKind::Interrupted,
                    "netlink route dump changed during enumeration",
                ));
            }
            match u16::from_ne_bytes(message[4..6].try_into().unwrap()) {
                3 => return Ok(interfaces),
                2 => {
                    let error = message.get(16..20).ok_or_else(invalid_link)?;
                    let error = i32::from_ne_bytes(error.try_into().unwrap());
                    if error != 0 {
                        return Err(io::Error::from_raw_os_error(-error));
                    }
                }
                16 => {
                    if let Some(interface) = parse_link(&message[16..])? {
                        interfaces.push(interface);
                    }
                }
                _ => {}
            }
            messages = messages
                .get(length.next_multiple_of(4)..)
                .ok_or_else(invalid_link)?;
        }
    }
}

fn invalid_link() -> std::io::Error {
    std::io::Error::new(
        std::io::ErrorKind::InvalidData,
        "invalid Linux interface snapshot",
    )
}

fn attributes(mut bytes: &[u8]) -> std::io::Result<Vec<(u16, &[u8])>> {
    let mut result = Vec::new();
    while !bytes.is_empty() {
        if bytes.len() < 4 {
            return Err(invalid_link());
        }
        let length = u16::from_ne_bytes(bytes[..2].try_into().unwrap()) as usize;
        if !(4..=bytes.len()).contains(&length) {
            return Err(invalid_link());
        }
        result.push((
            u16::from_ne_bytes(bytes[2..4].try_into().unwrap()) & 0x3fff,
            &bytes[4..length],
        ));
        bytes = bytes
            .get(length.next_multiple_of(4)..)
            .ok_or_else(invalid_link)?;
    }
    Ok(result)
}

fn parse_link(bytes: &[u8]) -> std::io::Result<Option<(String, u32)>> {
    if bytes.len() < 16 {
        return Err(invalid_link());
    }
    let link_type = u16::from_ne_bytes(bytes[2..4].try_into().unwrap());
    let index = u32::from_ne_bytes(bytes[4..8].try_into().unwrap());
    let flags = u32::from_ne_bytes(bytes[8..12].try_into().unwrap());
    // ARPHRD_ETHER、IFF_UP、IFF_RUNNING，排除回环和点对点链路。
    if link_type != 1 || flags & 0x41 != 0x41 || flags & (8 | 0x10) != 0 {
        return Ok(None);
    }
    let mut name = None;
    for (kind, value) in attributes(&bytes[16..])? {
        if kind == 3 {
            name = Some(
                std::str::from_utf8(value.strip_suffix(&[0]).unwrap_or(value))
                    .map_err(|_| invalid_link())?
                    .to_owned(),
            );
        } else if kind == 18 {
            for (nested_kind, value) in attributes(value)? {
                // 实体网卡无 IFLA_INFO_KIND；虚拟机、容器、桥和 VLAN 仍是有效的外网承载接口。
                if nested_kind == 1
                    && !matches!(
                        value.strip_suffix(&[0]).unwrap_or(value),
                        b"veth" | b"bridge" | b"bond" | b"team" | b"vlan" | b"macvlan" | b"ipvlan"
                    )
                {
                    return Ok(None);
                }
            }
        }
    }
    Ok(name.map(|name| (name, index)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vpn_routes_do_not_override_underlay_metric_or_family() {
        let v4 = "Iface Destination Gateway Flags RefCnt Use Metric Mask\n\
            tun0 00000000 00000000 0001 0 0 0 00000000\n\
            tun0 00000000 00000000 0001 0 0 0 00000080\n\
            eth0 00000000 0100000A 0003 0 0 100 00000000\n\
            wlan0 00000000 0100000A 0003 0 0 600 00000000\n\
            eth0 00000000 00000000 0201 0 0 0 00000000\n";
        let routes = default_routes(v4, false);
        assert_eq!(select_route(&routes, |name| name != "tun0"), Some("eth0"));
        assert_eq!(select_route(&routes, |name| name == "wlan0"), Some("wlan0"));
        assert_eq!(select_route(&routes, |_| false), None);
        let zero = "00000000000000000000000000000000";
        let v6 = format!(
            "{zero} 00 {zero} 00 fe800000000000000000000000000001 00000400 0 0 00000003 eth0\n\
            {zero} 00 {zero} 00 {zero} 00000000 0 0 00000201 lo\n\
            {zero} 01 {zero} 00 {zero} 00000000 0 0 00000001 tun0\n\
            {zero} 00 20010db8000000000000000000000000 40 {zero} 00000000 0 0 00000001 wlan0\n"
        );
        assert_eq!(default_routes(&v6, true), vec![("eth0", 1024)]);
    }

    #[test]
    fn malformed_or_down_routes_are_not_selected() {
        assert!(
            default_routes("garbage\neth0 00000000 00000000 0000 0 0 1 00000000", false).is_empty()
        );
        assert!(default_routes("bad ipv6 route", true).is_empty());
    }
}
