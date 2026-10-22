use std::{
    io,
    net::SocketAddr,
    os::windows::io::AsRawSocket,
    sync::LazyLock,
    time::{Duration, Instant},
};

use parking_lot::Mutex;
use windows::Win32::{
    NetworkManagement::{
        IpHelper::{
            FreeMibTable, GetIfEntry2, GetIpForwardTable2, GetIpInterfaceEntry, MIB_IF_ROW2,
            MIB_IPFORWARD_ROW2, MIB_IPINTERFACE_ROW,
        },
        Ndis::IfOperStatusUp,
    },
    Networking::WinSock::{ADDRESS_FAMILY, AF_INET, AF_INET6, SOCKET},
};

// 打洞会批量创建 socket，短期缓存避免逐个枚举路由；新建连接最多延迟两秒感知出口变化。
type InterfaceCache = [Option<(Instant, u32)>; 2];
static INTERFACES: LazyLock<Mutex<InterfaceCache>> = LazyLock::new(|| Mutex::new([None; 2]));
const REFRESH_INTERVAL: Duration = Duration::from_secs(2);

/// 为未指定源地址的底层 socket 约束物理出口，不修改全局路由或维护远端 IP 列表。
pub(crate) fn bind_socket<S: AsRawSocket>(
    socket: &S,
    local_addr: SocketAddr,
    bind_device: Option<&str>,
    need_protect: bool,
) -> io::Result<()> {
    // 显式网卡（含空字符串禁用绑定）、显式源地址和本机控制通信均保留调用方策略。
    if !need_protect || bind_device.is_some() || !local_addr.ip().is_unspecified() {
        return Ok(());
    }
    let family = if local_addr.is_ipv4() {
        AF_INET
    } else {
        AF_INET6
    };
    let index = physical_default_interface(family)?;
    super::set_ip_unicast_if_index(SOCKET(socket.as_raw_socket() as usize), &local_addr, index)
}

pub(crate) fn physical_default_interface(family: ADDRESS_FAMILY) -> io::Result<u32> {
    let slot = usize::from(family == AF_INET6);
    let mut cache = INTERFACES.lock();
    if let Some((checked_at, index)) = cache[slot]
        && checked_at.elapsed() < REFRESH_INTERVAL
    {
        return Ok(index);
    }

    let mut table = std::ptr::null_mut();
    unsafe { GetIpForwardTable2(family, &mut table) }.ok()?;
    // Windows 返回变长表；切片只在释放系统内存前使用，后续接口查询均不提前返回。
    let routes = unsafe {
        std::slice::from_raw_parts((*table).Table.as_ptr(), (*table).NumEntries as usize)
    };
    let selected = select_interface(routes, |index| physical_interface_metric(index, family));
    unsafe { FreeMibTable(table.cast()) };

    let index = selected.ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::NetworkUnreachable,
            format!(
                "EasyTier underlay: no active physical default route for {family:?}; check the network or configure bind_address explicitly"
            ),
        )
    })?;
    if cache[slot].is_none_or(|(_, previous)| previous != index) {
        tracing::info!(
            ?family,
            interface_index = index,
            "EasyTier underlay selected physical default interface; STUN and peer sockets bypass VPN routes"
        );
    }
    cache[slot] = Some((Instant::now(), index));
    Ok(index)
}

fn physical_interface_metric(index: u32, family: ADDRESS_FAMILY) -> Option<u32> {
    let mut interface = MIB_IF_ROW2 {
        InterfaceIndex: index,
        ..Default::default()
    };
    unsafe { GetIfEntry2(&mut interface) }.ok().ok()?;
    // HardwareInterface 是系统定义的第零位，避免用网卡名称或 IP 段猜测物理网卡。
    if interface.InterfaceAndOperStatusFlags._bitfield & 1 == 0
        || interface.OperStatus != IfOperStatusUp
    {
        return None;
    }
    let mut ip_interface = MIB_IPINTERFACE_ROW {
        Family: family,
        InterfaceIndex: index,
        ..Default::default()
    };
    unsafe { GetIpInterfaceEntry(&mut ip_interface) }
        .ok()
        .ok()?;
    ip_interface.Connected.then_some(ip_interface.Metric)
}

fn select_interface(
    routes: &[MIB_IPFORWARD_ROW2],
    mut interface_metric: impl FnMut(u32) -> Option<u32>,
) -> Option<u32> {
    routes
        .iter()
        .filter(|route| {
            route.DestinationPrefix.PrefixLength == 0 && route.ValidLifetime != 0 && !route.Loopback
        })
        .filter_map(|route| {
            let metric = interface_metric(route.InterfaceIndex)?;
            Some((
                u64::from(route.Metric) + u64::from(metric),
                route.InterfaceIndex,
            ))
        })
        .min()
        .map(|(_, index)| index)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn route(index: u32, metric: u32, prefix: u8) -> MIB_IPFORWARD_ROW2 {
        let mut route = MIB_IPFORWARD_ROW2 {
            InterfaceIndex: index,
            Metric: metric,
            ValidLifetime: u32::MAX,
            ..Default::default()
        };
        route.DestinationPrefix.PrefixLength = prefix;
        route
    }

    #[test]
    fn physical_default_route_ignores_vpn_routes_and_uses_total_metric() {
        let routes = [
            route(50, 0, 0),
            route(50, 0, 1),
            route(2, 10, 0),
            route(3, 5, 0),
        ];
        assert_eq!(
            select_interface(&routes, |index| match index {
                2 => Some(5),
                3 => Some(20),
                _ => None,
            }),
            Some(2)
        );
        // 出口切换后重新选择仍在线的物理网卡，不依赖之前的 IP 或接口编号。
        assert_eq!(
            select_interface(&routes, |index| (index == 3).then_some(20)),
            Some(3)
        );
    }

    #[test]
    fn no_physical_default_route_does_not_select_vpn_or_expired_routes() {
        let mut expired = route(2, 0, 0);
        expired.ValidLifetime = 0;
        assert_eq!(
            select_interface(&[route(50, 0, 0), route(2, 0, 24), expired], |index| {
                (index == 2).then_some(5)
            }),
            None
        );
    }
}
