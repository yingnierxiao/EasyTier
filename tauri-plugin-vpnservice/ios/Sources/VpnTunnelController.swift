import Foundation
import NetworkExtension

/// 沿用 VPN 项目的系统授权与配置保存流程，异步等待真实连接状态。
@MainActor
final class VpnTunnelController {
    private var manager: NETunnelProviderManager?
    private var connectionTask: Task<[String: Any], Error>?
    private var providerID: String { (Bundle.main.bundleIdentifier ?? "com.yingnierxiao.easytier") + ".PacketTunnel" }

    func connectCore() async throws -> [String: Any] {
        if let task = connectionTask { return try await task.value }
        let task = Task { @MainActor in try await self.establishConnection() }
        connectionTask = task
        defer { connectionTask = nil }
        return try await task.value
    }

    private func establishConnection() async throws -> [String: Any] {
        let managers = try await NETunnelProviderManager.loadAllFromPreferences()
        let selected = managers.first { ($0.protocolConfiguration as? NETunnelProviderProtocol)?.providerBundleIdentifier == providerID } ?? NETunnelProviderManager()
        manager = selected
        if selected.connection.status != .connected && selected.connection.status != .connecting && selected.connection.status != .reasserting {
            let proto = NETunnelProviderProtocol()
            proto.providerBundleIdentifier = providerID
            proto.serverAddress = "EasyTier"
            selected.protocolConfiguration = proto
            selected.localizedDescription = "EasyTier"
            selected.isEnabled = true
            try await selected.saveToPreferences()
            try await selected.loadFromPreferences()
            try selected.connection.startVPNTunnel()
        }
        let started = Date()
        let deadline = started.addingTimeInterval(25)
        var sawConnecting = false
        while selected.connection.status != .connected && selected.connection.status != .reasserting {
            if Date() >= deadline { throw failure("VPN 启动超时，请检查系统 VPN 状态") }
            if selected.connection.status == .connecting { sawConnecting = true }
            if selected.connection.status == .invalid || (selected.connection.status == .disconnected && (sawConnecting || Date().timeIntervalSince(started) > 1)) {
                if #available(iOS 16.0, *) { try await selected.connection.fetchLastDisconnectError() }
                throw failure("VPN 扩展启动失败")
            }
            try await Task.sleep(nanoseconds: 100_000_000)
        }
        return try await request(["command": "endpoint"])
    }

    func request(_ request: [String: Any]) async throws -> [String: Any] {
        guard let session = manager?.connection as? NETunnelProviderSession else { throw failure("VPN 管理连接尚未建立") }
        let data = try JSONSerialization.data(withJSONObject: request)
        return try await withCheckedThrowingContinuation { continuation in
            var completed = false
            let finish: (Result<[String: Any], Error>) -> Void = { result in
                DispatchQueue.main.async {
                    guard !completed else { return }
                    completed = true
                    continuation.resume(with: result)
                }
            }
            do {
                try session.sendProviderMessage(data) { data in
                    do {
                        guard let data, let envelope = try JSONSerialization.jsonObject(with: data) as? [String: Any] else { throw self.failure("扩展未返回管理结果") }
                        guard envelope["ok"] as? Bool == true else { throw self.failure(envelope["error"] as? String ?? "扩展操作失败") }
                        finish(.success(envelope["value"] as? [String: Any] ?? [:]))
                    } catch { finish(.failure(error)) }
                }
            } catch { finish(.failure(error)) }
            DispatchQueue.main.asyncAfter(deadline: .now() + 8) { finish(.failure(self.failure("扩展管理请求超时"))) }
        }
    }

    func stop() { manager?.connection.stopVPNTunnel() }
    var running: Bool { manager?.connection.status == .connected || manager?.connection.status == .reasserting }

    private func failure(_ message: String) -> NSError {
        NSError(domain: "EasyTier.VPN", code: 1, userInfo: [NSLocalizedDescriptionKey: message])
    }
}
