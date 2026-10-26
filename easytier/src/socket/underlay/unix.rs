use std::{
    io,
    net::SocketAddr,
    os::fd::AsFd,
    sync::{Arc, LazyLock},
    time::{Duration, Instant},
};

use parking_lot::Mutex;

/// 出口按接口身份缓存，不缓存会变化的本机地址或对端地址。
pub(super) struct Interface {
    pub name: String,
    pub index: u32,
}

type InterfaceCache = [Option<(u64, Instant, Arc<Interface>)>; 2];
static INTERFACES: LazyLock<Mutex<InterfaceCache>> = LazyLock::new(|| Mutex::new([None, None]));
const REFRESH_INTERVAL: Duration = Duration::from_secs(2);

/// 使用逐包接口元数据，保留同一端口上的回环接收、NAT 映射和并发发送能力。
pub(super) fn send_to(
    socket: &tokio::net::UdpSocket,
    data: &[u8],
    destination: SocketAddr,
    index: u32,
) -> io::Result<usize> {
    use nix::libc;
    use std::{mem, os::fd::AsRawFd, ptr};

    #[repr(align(8))]
    struct ControlBuffer([u8; 128]);

    let destination_address = socket2::SockAddr::from(destination);
    let mut iov = libc::iovec {
        iov_base: data.as_ptr() as *mut libc::c_void,
        iov_len: data.len(),
    };
    let ipv6 = destination.ip().to_canonical().is_ipv6();
    let payload_len = if ipv6 {
        mem::size_of::<libc::in6_pktinfo>()
    } else {
        mem::size_of::<libc::in_pktinfo>()
    };
    let mut control = ControlBuffer([0; 128]);
    let mut message: libc::msghdr = unsafe { mem::zeroed() };
    message.msg_name = destination_address.as_ptr() as *mut libc::c_void;
    message.msg_namelen = destination_address.len();
    message.msg_iov = &mut iov;
    message.msg_iovlen = 1;
    message.msg_control = control.0.as_mut_ptr().cast();
    message.msg_controllen = unsafe { libc::CMSG_SPACE(payload_len as _) } as _;
    unsafe {
        let header = libc::CMSG_FIRSTHDR(&message);
        (*header).cmsg_len = libc::CMSG_LEN(payload_len as _) as _;
        if ipv6 {
            (*header).cmsg_level = libc::IPPROTO_IPV6;
            (*header).cmsg_type = libc::IPV6_PKTINFO;
            let mut info: libc::in6_pktinfo = mem::zeroed();
            info.ipi6_ifindex = index;
            ptr::write(libc::CMSG_DATA(header).cast::<libc::in6_pktinfo>(), info);
        } else {
            (*header).cmsg_level = libc::IPPROTO_IP;
            (*header).cmsg_type = libc::IP_PKTINFO;
            let mut info: libc::in_pktinfo = mem::zeroed();
            info.ipi_ifindex = index as _;
            ptr::write(libc::CMSG_DATA(header).cast::<libc::in_pktinfo>(), info);
        }
        let sent = libc::sendmsg(socket.as_raw_fd(), &message, 0);
        if sent < 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(sent as usize)
        }
    }
}

pub(super) fn bind_socket<S: AsFd>(socket: &S, local_addr: SocketAddr) -> io::Result<()> {
    let interface = default_interface(local_addr.is_ipv6())?;
    let socket = socket2::SockRef::from(socket);
    #[cfg(target_os = "linux")]
    socket.bind_device(Some(interface.name.as_bytes()))?;
    #[cfg(target_os = "macos")]
    {
        let index = std::num::NonZeroU32::new(interface.index).ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                "invalid underlay interface index",
            )
        })?;
        if local_addr.is_ipv4() {
            socket.bind_device_by_index_v4(Some(index))?;
        } else {
            socket.bind_device_by_index_v6(Some(index))?;
        }
    }
    Ok(())
}

pub(super) fn default_interface(ipv6: bool) -> io::Result<Arc<Interface>> {
    #[cfg(target_os = "linux")]
    let namespace = {
        use std::os::unix::fs::MetadataExt;
        // 查询发生在 socket 所属命名空间内；线程切换命名空间后不能复用其他空间的缓存。
        std::fs::metadata("/proc/thread-self/ns/net")?.ino()
    };
    #[cfg(target_os = "macos")]
    let namespace = 0;
    let mut cache = INTERFACES.lock();
    let slot = usize::from(ipv6);
    if let Some((cached_namespace, checked_at, interface)) = &cache[slot]
        && *cached_namespace == namespace
        && checked_at.elapsed() < REFRESH_INTERVAL
    {
        return Ok(interface.clone());
    }
    #[cfg(target_os = "linux")]
    let interface = super::linux::default_interface(ipv6)?;
    #[cfg(target_os = "macos")]
    let interface = super::macos::default_interface(ipv6)?;
    if cache[slot]
        .as_ref()
        .is_none_or(|(previous_ns, _, previous)| {
            *previous_ns != namespace || previous.index != interface.index
        })
    {
        tracing::info!(
            ipv6,
            interface = interface.name,
            interface_index = interface.index,
            "EasyTier underlay selected default interface; STUN and peer sockets bypass VPN routes"
        );
    }
    let interface = Arc::new(interface);
    cache[slot] = Some((namespace, Instant::now(), interface.clone()));
    Ok(interface)
}

pub(super) fn no_default_route(ipv6: bool) -> io::Error {
    io::Error::new(
        io::ErrorKind::NetworkUnreachable,
        format!(
            "EasyTier underlay: no active non-tunnel IPv{} default route; check the network or configure bind_address explicitly",
            if ipv6 { 6 } else { 4 }
        ),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::net::UdpSocket;

    #[cfg(target_os = "linux")]
    #[tokio::test]
    #[ignore = "requires an active underlay default route and permission to bind an interface"]
    async fn default_underlay_sets_native_socket_option() {
        use std::os::fd::AsRawFd;
        let interface = default_interface(false).unwrap();
        let socket = UdpSocket::bind("0.0.0.0:0").await.unwrap();
        bind_socket(&socket, socket.local_addr().unwrap()).unwrap();
        let mut name = [0u8; 64];
        let mut length = name.len() as nix::libc::socklen_t;
        assert_eq!(
            unsafe {
                nix::libc::getsockopt(
                    socket.as_raw_fd(),
                    nix::libc::SOL_SOCKET,
                    nix::libc::SO_BINDTODEVICE,
                    name.as_mut_ptr().cast(),
                    &mut length,
                )
            },
            0
        );
        assert_eq!(&name[..length as usize - 1], interface.name.as_bytes());
    }

    #[tokio::test]
    async fn packet_interface_preserves_listener_port_and_loopback_control() {
        for address in ["127.0.0.1:0", "[::1]:0"] {
            let receiver = UdpSocket::bind(address).await.unwrap();
            let destination = receiver.local_addr().unwrap();
            let shared = UdpSocket::bind(if destination.is_ipv6() {
                "[::]:0"
            } else {
                "0.0.0.0:0"
            })
            .await
            .unwrap();
            let index = nix::net::if_::if_nametoindex(if cfg!(target_os = "macos") {
                "lo0"
            } else {
                "lo"
            })
            .unwrap();
            shared
                .async_io(tokio::io::Interest::WRITABLE, || {
                    send_to(&shared, b"hole-punch", destination, index)
                })
                .await
                .unwrap();
            let mut buffer = [0; 32];
            let (length, source) =
                tokio::time::timeout(Duration::from_secs(1), receiver.recv_from(&mut buffer))
                    .await
                    .unwrap()
                    .unwrap();
            assert_eq!(&buffer[..length], b"hole-punch");
            assert_eq!(source.port(), shared.local_addr().unwrap().port());
            receiver.send_to(b"control", source).await.unwrap();
            let (length, _) =
                tokio::time::timeout(Duration::from_secs(1), shared.recv_from(&mut buffer))
                    .await
                    .unwrap()
                    .unwrap();
            assert_eq!(&buffer[..length], b"control");
        }
    }
}
