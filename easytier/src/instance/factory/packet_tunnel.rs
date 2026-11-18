use super::NativeInstanceManager;
use crate::instance::runtime_host::NativeInstanceRuntimeHost;
use easytier_core::{config::toml::ConfigLoader, instance::CoreInstanceState};
use std::collections::BTreeSet;

#[derive(serde::Serialize)]
pub struct PacketTunnelSnapshot {
    pub addresses: Vec<String>,
    pub routes: Vec<String>,
    pub dns: Option<String>,
    pub mtu: u32,
}

/// 从扩展内的真实运行状态生成设置；没有路由时不接管默认路由。
pub async fn snapshot(
    manager: &NativeInstanceManager,
    fd: i32,
    attached: &mut Option<(
        std::sync::Weak<super::NativeCoreInstance>,
        Vec<String>,
        bool,
        u32,
    )>,
) -> anyhow::Result<PacketTunnelSnapshot> {
    let mut settings = PacketTunnelSnapshot {
        addresses: Vec::new(),
        routes: Vec::new(),
        dns: None,
        mtu: 1380,
    };
    let mut routes = BTreeSet::new();
    let mut active = None;
    for id in manager.instance_ids() {
        let Some(config) = manager.config(id) else {
            continue;
        };
        if config.get_flags().no_tun {
            continue;
        }
        let Some(instance) = manager.instance(id) else {
            continue;
        };
        if instance.state() != CoreInstanceState::Running {
            continue;
        }
        anyhow::ensure!(active.is_none(), "iOS 当前仅支持一个启用虚拟网卡的网络");
        active = Some(id);
        let host = instance
            .runtime_host::<NativeInstanceRuntimeHost>()
            .ok_or_else(|| anyhow::anyhow!("native runtime unavailable"))?;
        settings.addresses = host.packet_tunnel_addresses();
        let unchanged = attached
            .as_ref()
            .is_some_and(|(previous, addresses, dns, mtu)| {
                previous
                    .upgrade()
                    .is_some_and(|previous| std::sync::Arc::ptr_eq(&previous, &instance))
                    && addresses == &settings.addresses
                    && *dns == config.get_flags().accept_dns
                    && *mtu == config.get_flags().mtu
            });
        if !unchanged {
            host.attach_packet_tunnel(fd).await?;
            *attached = Some((
                std::sync::Arc::downgrade(&instance),
                settings.addresses.clone(),
                config.get_flags().accept_dns,
                config.get_flags().mtu,
            ));
        }
        settings.mtu = config.get_flags().mtu;
        for address in &settings.addresses {
            let address: cidr::IpInet = address.parse()?;
            routes.insert(address.network().to_string());
        }
        if let Some(manual) = config.get_routes() {
            routes.extend(manual.into_iter().map(|route| route.to_string()));
        } else {
            // 只读取路由，避免定时复制连接、事件和节点状态等完整管理快照。
            for route in instance.route_snapshots().await {
                if let Some(address) = route.ipv4_addr.and_then(|inet| inet.address) {
                    let address: std::net::Ipv4Addr = address.into();
                    routes.insert(format!("{address}/32"));
                }
                if let Some(address) = route.ipv6_addr.and_then(|inet| inet.address) {
                    let address: std::net::Ipv6Addr = address.into();
                    routes.insert(format!("{address}/128"));
                }
                for network in route.proxy_cidrs {
                    let network: cidr::IpCidr = network.parse()?;
                    routes.insert(network.to_string());
                }
            }
        }
        if !config.get_exit_nodes().is_empty() {
            routes.insert("0.0.0.0/0".to_owned());
        }
        if config.get_flags().accept_dns
            && settings
                .addresses
                .iter()
                .any(|address| !address.contains(':'))
        {
            let dns = crate::instance::dns_server::MAGIC_DNS_FAKE_IP;
            routes.insert(format!("{dns}/32"));
            settings.dns = Some(dns.to_owned());
        }
    }
    if active.is_none() {
        *attached = None;
    }
    settings.routes = routes.into_iter().collect();
    Ok(settings)
}
