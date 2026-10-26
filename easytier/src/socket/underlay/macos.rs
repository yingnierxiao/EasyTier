use std::io;

// Darwin 的 rt_msghdr ABI 在 Intel、Apple Silicon 上均为 92 字节；原生测试核对 libc 定义。
const HEADER_LEN: usize = 92;

/// 安全解析 sysctl 路由快照，只返回可转发的默认路由和接口优先级。
fn default_routes(mut bytes: &[u8], ipv6: bool) -> io::Result<Vec<(u32, u32)>> {
    let mut routes = Vec::new();
    while !bytes.is_empty() {
        if bytes.len() < HEADER_LEN {
            return Err(invalid_route());
        }
        let length = u16::from_ne_bytes([bytes[0], bytes[1]]) as usize;
        if !(HEADER_LEN..=bytes.len()).contains(&length) || bytes[2] != 5 {
            return Err(invalid_route());
        }
        let message = &bytes[..length];
        bytes = &bytes[length..];
        let index = u16::from_ne_bytes([message[4], message[5]]) as u32;
        let flags = u32::from_ne_bytes(message[8..12].try_into().unwrap());
        let addresses = u32::from_ne_bytes(message[12..16].try_into().unwrap());
        // RTF_UP、RTF_HOST、RTF_REJECT、RTF_BLACKHOLE；默认地址为零的主机路由也不能选中。
        if flags & 1 == 0 || flags & (4 | 8 | 0x1000) != 0 || addresses & 1 == 0 {
            continue;
        }
        let mut cursor = HEADER_LEN;
        let mut destination = None;
        let mut netmask = None;
        for slot in 0..8 {
            if addresses & (1 << slot) == 0 {
                continue;
            }
            let length = *message.get(cursor).ok_or_else(invalid_route)? as usize;
            // Darwin 的路由 sockaddr 按四字节对齐，零长度掩码仍占四字节。
            let aligned = length.max(1).div_ceil(4) * 4;
            let address = message
                .get(cursor..cursor + aligned)
                .ok_or_else(invalid_route)?;
            if slot == 0 {
                destination = Some(address);
            } else if slot == 2 {
                netmask = Some(address);
            }
            cursor += aligned;
        }
        let Some(destination) = destination else {
            continue;
        };
        let (family, offset, address_len) = if ipv6 { (30, 8, 16) } else { (2, 4, 4) };
        if destination.get(1) != Some(&family)
            || destination
                .get(offset..offset + address_len)
                .is_none_or(|ip| ip.iter().any(|&v| v != 0))
        {
            continue;
        }
        // 没有掩码的是主机路由；压缩掩码未包含的尾部字节按零处理。
        let Some(netmask) = netmask else { continue };
        let mask_len = usize::from(netmask[0]);
        if netmask
            .get(offset..mask_len)
            .is_some_and(|mask| mask.iter().any(|&v| v != 0))
        {
            continue;
        }
        // 优先使用系统未限定接口的默认路由；VPN 接管后，仍可选择物理接口的 scoped 默认路由。
        let scoped = u32::from(flags & 0x1000000 != 0);
        routes.push((index, scoped));
    }
    Ok(routes)
}

fn invalid_route() -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        "invalid Darwin routing table snapshot",
    )
}

#[cfg(target_os = "macos")]
pub(super) fn default_interface(ipv6: bool) -> io::Result<super::unix::Interface> {
    use nix::{ifaddrs::getifaddrs, libc, net::if_::InterfaceFlags};

    let mut mib = [
        libc::CTL_NET,
        libc::PF_ROUTE,
        0,
        if ipv6 { libc::AF_INET6 } else { libc::AF_INET },
        libc::NET_RT_DUMP,
        0,
    ];
    let mut length = 0;
    // 直接读取内核快照，避免启动 route/netstat 子进程和本地化文本解析。
    let read = |buffer: *mut libc::c_void, length: &mut usize| -> io::Result<()> {
        let result =
            unsafe { libc::sysctl(mib.as_mut_ptr(), 6, buffer, length, std::ptr::null_mut(), 0) };
        if result == -1 {
            Err(io::Error::last_os_error())
        } else {
            Ok(())
        }
    };
    let mut read = read;
    read(std::ptr::null_mut(), &mut length)?;
    let mut bytes = vec![0; length];
    read(bytes.as_mut_ptr().cast(), &mut length)?;
    bytes.truncate(length);
    let routes = default_routes(&bytes, ipv6)?;
    let interfaces: Vec<_> = getifaddrs()?
        .filter_map(|interface| {
            if !interface
                .flags
                .contains(InterfaceFlags::IFF_UP | InterfaceFlags::IFF_RUNNING)
                || interface
                    .flags
                    .intersects(InterfaceFlags::IFF_LOOPBACK | InterfaceFlags::IFF_POINTOPOINT)
            {
                return None;
            }
            let address = interface.address?;
            let link = address.as_link_addr()?;
            // 使用内核链路类型，排除 utun、PPP、回环；兼容有线和无线网卡。
            if !matches!(link.datalink_type(), 6 | 71) {
                return None;
            }
            Some(super::unix::Interface {
                name: interface.interface_name,
                index: link.ifindex() as u32,
            })
        })
        .collect();
    routes
        .iter()
        .filter_map(|(index, priority)| {
            interfaces
                .iter()
                .find(|interface| interface.index == *index)
                .map(|interface| (*priority, interface))
        })
        .min_by_key(|(priority, interface)| (*priority, interface.index))
        .map(|(_, interface)| super::unix::Interface {
            name: interface.name.clone(),
            index: interface.index,
        })
        .ok_or_else(|| super::unix::no_default_route(ipv6))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn route(index: u16, ipv6: bool, scoped: bool, mask: u8) -> Vec<u8> {
        let address_len = if ipv6 { 28 } else { 16 };
        let mut bytes = vec![0; HEADER_LEN + address_len + 4];
        let length = bytes.len() as u16;
        bytes[..2].copy_from_slice(&length.to_ne_bytes());
        bytes[2] = 5;
        bytes[4..6].copy_from_slice(&index.to_ne_bytes());
        let flags: u32 = 1 | if scoped { 0x1000000 } else { 0 };
        bytes[8..12].copy_from_slice(&flags.to_ne_bytes());
        bytes[12..16].copy_from_slice(&5u32.to_ne_bytes());
        bytes[HEADER_LEN] = address_len as u8;
        bytes[HEADER_LEN + 1] = if ipv6 { 30 } else { 2 };
        if mask != 0 {
            // 测试用非默认掩码重新分配完整 sockaddr。
            bytes.resize(HEADER_LEN + address_len * 2, 0);
            let length = bytes.len() as u16;
            bytes[..2].copy_from_slice(&length.to_ne_bytes());
            bytes[HEADER_LEN + address_len] = address_len as u8;
            bytes[HEADER_LEN + address_len + if ipv6 { 8 } else { 4 }] = mask;
        }
        bytes
    }

    #[test]
    fn scoped_defaults_survive_vpn_and_non_default_routes_are_ignored() {
        for ipv6 in [false, true] {
            let mut bytes = route(50, ipv6, false, 0);
            bytes.extend(route(6, ipv6, true, 0));
            bytes.extend(route(6, ipv6, true, 128));
            let routes = default_routes(&bytes, ipv6).unwrap();
            assert_eq!(routes, vec![(50, 0), (6, 1)]);
            // 只有内核确认的实际联网接口才能参与选择，VPN 的默认路由不会覆盖物理出口。
            assert_eq!(
                routes.into_iter().filter(|(index, _)| *index == 6).min(),
                Some((6, 1))
            );
            assert!(default_routes(&bytes, !ipv6).unwrap().is_empty());
        }
    }

    #[test]
    fn truncated_routes_fail_without_panicking() {
        let bytes = route(6, false, false, 0);
        for end in 1..bytes.len() {
            assert!(default_routes(&bytes[..end], false).is_err());
        }
        let mut invalid = bytes;
        invalid[0] = 0;
        invalid[1] = 0;
        assert!(default_routes(&invalid, false).is_err());
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn routing_header_matches_darwin_abi() {
        assert_eq!(HEADER_LEN, std::mem::size_of::<nix::libc::rt_msghdr>());
    }
}
