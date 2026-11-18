import Darwin
import Foundation

struct TunnelNetwork {
    let address: String
    let prefix: Int
    let ipv6: Bool

    init(_ cidr: String) throws {
        let parts = cidr.split(separator: "/", omittingEmptySubsequences: false)
        guard parts.count == 2, let prefix = Int(parts[1]) else { throw Self.invalid(cidr) }
        address = String(parts[0]); self.prefix = prefix; ipv6 = address.contains(":")
        var bytes = in6_addr()
        guard (0...(ipv6 ? 128 : 32)).contains(prefix), inet_pton(ipv6 ? AF_INET6 : AF_INET, address, &bytes) == 1 else { throw Self.invalid(cidr) }
    }

    var mask: String {
        let bits: UInt32 = prefix == 0 ? 0 : UInt32.max << (32 - prefix)
        return [24, 16, 8, 0].map { String((bits >> $0) & 255) }.joined(separator: ".")
    }

    var networkAddress: String {
        var bytes = [UInt8](repeating: 0, count: ipv6 ? 16 : 4)
        _ = bytes.withUnsafeMutableBytes { inet_pton(ipv6 ? AF_INET6 : AF_INET, address, $0.baseAddress) }
        for index in bytes.indices {
            let retained = min(8, max(0, prefix - index * 8))
            bytes[index] &= retained == 0 ? 0 : UInt8(truncatingIfNeeded: 255 << (8 - retained))
        }
        var output = [CChar](repeating: 0, count: Int(INET6_ADDRSTRLEN))
        _ = bytes.withUnsafeBytes { inet_ntop(ipv6 ? AF_INET6 : AF_INET, $0.baseAddress, &output, socklen_t(output.count)) }
        return String(cString: output)
    }

    private static func invalid(_ cidr: String) -> NSError {
        NSError(domain: "EasyTier.PacketTunnel", code: 1, userInfo: [NSLocalizedDescriptionKey: "无效的网络地址：\(cidr)"])
    }
}
