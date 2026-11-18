import Foundation
import NetworkExtension

struct TunnelSnapshot: Decodable, Equatable {
    let addresses: [String]
    let routes: [String]
    let dns: String?
    let mtu: UInt32

    func networkSettings() throws -> NEPacketTunnelNetworkSettings {
        guard (576...9000).contains(mtu) else { throw NSError(domain: "EasyTier.PacketTunnel", code: 2, userInfo: [NSLocalizedDescriptionKey: "无效的 MTU"]) }
        let addresses = try addresses.map(TunnelNetwork.init)
        let routes = try routes.map(TunnelNetwork.init)
        let settings = NEPacketTunnelNetworkSettings(tunnelRemoteAddress: "127.0.0.1")
        settings.mtu = NSNumber(value: mtu)
        let v4 = addresses.filter { !$0.ipv6 }
        // 空闲时只保留管理连接，不安装默认路由或修改系统 DNS。
        let ipv4 = NEIPv4Settings(addresses: v4.isEmpty ? ["192.0.2.1"] : v4.map(\.address), subnetMasks: v4.isEmpty ? ["255.255.255.255"] : v4.map(\.mask))
        ipv4.includedRoutes = routes.filter { !$0.ipv6 }.map { NEIPv4Route(destinationAddress: $0.networkAddress, subnetMask: $0.mask) }
        settings.ipv4Settings = ipv4
        let v6 = addresses.filter(\.ipv6)
        if !v6.isEmpty {
            let ipv6 = NEIPv6Settings(addresses: v6.map(\.address), networkPrefixLengths: v6.map { NSNumber(value: $0.prefix) })
            ipv6.includedRoutes = routes.filter(\.ipv6).map { NEIPv6Route(destinationAddress: $0.networkAddress, networkPrefixLength: NSNumber(value: $0.prefix)) }
            settings.ipv6Settings = ipv6
        }
        if let dns, !addresses.isEmpty {
            let resolver = NEDNSSettings(servers: [dns])
            resolver.matchDomains = [""]
            resolver.matchDomainsNoSearch = true
            settings.dnsSettings = resolver
        }
        return settings
    }
}
