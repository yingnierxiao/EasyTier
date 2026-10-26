//! 桌面端底层连接的出口约束，保留显式源地址、显式网卡和本机通信策略。

#[cfg(any(target_os = "linux", target_os = "macos"))]
mod unix;

#[cfg(any(target_os = "linux", test))]
mod linux;
#[cfg(any(target_os = "macos", test))]
mod macos;

#[cfg(unix)]
use std::os::fd::AsFd as SocketHandle;
#[cfg(windows)]
use std::os::windows::io::AsRawSocket as SocketHandle;
use std::{io, net::SocketAddr};

use easytier_core::socket::SocketContext;

use crate::common::netns::NetNS;

#[cfg(any(target_os = "linux", target_os = "macos"))]
pub(crate) async fn send_to(
    socket: &tokio::net::UdpSocket,
    data: &[u8],
    destination: SocketAddr,
    context: &SocketContext,
) -> io::Result<usize> {
    let interface = NetNS::from_socket_context(context)
        .run(|| unix::default_interface(destination.ip().to_canonical().is_ipv6()))?;
    socket
        .async_io(tokio::io::Interest::WRITABLE, || {
            unix::send_to(socket, data, destination, interface.index)
        })
        .await
}

pub(crate) fn bind_socket<S: SocketHandle>(
    socket: &S,
    local_addr: SocketAddr,
    bind_device: Option<&str>,
    need_protect: bool,
    context: &SocketContext,
) -> io::Result<()> {
    // 显式指定空网卡也表示禁用绑定，不能被自动出口选择覆盖。
    if !need_protect || bind_device.is_some() || !local_addr.ip().is_unspecified() {
        return Ok(());
    }
    NetNS::from_socket_context(context).run(|| {
        #[cfg(target_os = "windows")]
        return crate::arch::windows::underlay::bind_socket(
            socket,
            local_addr,
            bind_device,
            need_protect,
        );
        #[cfg(any(target_os = "linux", target_os = "macos"))]
        return unix::bind_socket(socket, local_addr);
        #[cfg(not(any(target_os = "windows", target_os = "linux", target_os = "macos")))]
        {
            let _ = socket;
            Ok(())
        }
    })
}
