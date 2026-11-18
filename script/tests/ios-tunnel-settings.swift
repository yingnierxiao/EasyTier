import Foundation
import NetworkExtension

@main
struct TunnelSettingsTests {
    static func tryNetwork(_ cidr: String) -> TunnelNetwork { try! TunnelNetwork(cidr) }
    static func main() throws {
        let v4 = try TunnelNetwork("10.123.45.67/24")
        precondition(v4.networkAddress == "10.123.45.0" && v4.mask == "255.255.255.0")
        precondition(tryNetwork("1.2.3.4/0").networkAddress == "0.0.0.0")
        precondition(tryNetwork("1.2.3.4/32").mask == "255.255.255.255")
        precondition(tryNetwork("fd12:3456::abcd/64").networkAddress == "fd12:3456::")
        precondition(tryNetwork("fd12:3456::abcd/128").networkAddress == "fd12:3456::abcd")
        for invalid in ["bad/24", "10.0.0.1/33", "fd00::1/129", "127.0.0.1", "10.0.0.1/-1"] {
            do { _ = try TunnelNetwork(invalid); fatalError("invalid CIDR accepted") } catch {}
        }
        let idle = try TunnelSnapshot(addresses: [], routes: [], dns: nil, mtu: 1380).networkSettings()
        precondition(idle.ipv4Settings?.includedRoutes?.isEmpty == true && idle.dnsSettings == nil)
        let active = try TunnelSnapshot(addresses: ["10.0.0.5/24", "fd00::5/64"], routes: ["10.0.0.0/24", "192.168.50.0/24", "fd00::/64"], dns: "100.100.100.100", mtu: 1380).networkSettings()
        precondition(active.ipv4Settings?.addresses == ["10.0.0.5"])
        precondition(active.ipv4Settings?.includedRoutes?.count == 2)
        precondition(active.ipv4Settings?.includedRoutes?.allSatisfy { $0.destinationSubnetMask != "0.0.0.0" } == true)
        precondition(active.ipv6Settings?.includedRoutes?.first?.destinationAddress == "fd00::")
        precondition(active.dnsSettings?.servers == ["100.100.100.100"])
        print("iOS tunnel CIDR, idle routing, split routing and DNS tests passed")
    }
}
