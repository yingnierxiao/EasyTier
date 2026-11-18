import Foundation
import NetworkExtension
import os.log

final class PacketTunnelProvider: NEPacketTunnelProvider {
    private let queue = DispatchQueue(label: "easytier.packet-tunnel.control")
    private let logger = Logger(subsystem: "com.yingnierxiao.easytier.PacketTunnel", category: "runtime")
    private var bridge: PacketFlowBridge?
    private var timer: DispatchSourceTimer?
    private var endpoint: [String: Any]?
    private var applied: TunnelSnapshot?
    private var applying = false
    private var generation: UInt64 = 0

    private func result(_ pointer: UnsafeMutablePointer<CChar>?) throws -> Any {
        guard let pointer else { throw failure("核心未返回结果") }
        defer { easytier_ios_free_string(pointer) }
        let data = Data(String(cString: pointer).utf8)
        guard let envelope = try JSONSerialization.jsonObject(with: data) as? [String: Any] else { throw failure("核心返回格式错误") }
        guard envelope["ok"] as? Bool == true else { throw failure(envelope["error"] as? String ?? "核心操作失败") }
        return envelope["value"] ?? NSNull()
    }

    private func snapshot() throws -> TunnelSnapshot {
        guard let bridge else { throw failure("数据通道尚未启动") }
        let value = try result(easytier_ios_packet_tunnel_snapshot(bridge.coreFD))
        return try JSONDecoder().decode(TunnelSnapshot.self, from: JSONSerialization.data(withJSONObject: value))
    }

    override func startTunnel(options: [String: NSObject]?, completionHandler: @escaping (Error?) -> Void) {
        queue.async { [self] in
            do {
                generation &+= 1
                let current = generation
                bridge = try PacketFlowBridge(flow: packetFlow)
                let directory = try FileManager.default.url(for: .applicationSupportDirectory, in: .userDomainMask, appropriateFor: nil, create: true).appendingPathComponent("EasyTier", isDirectory: true)
                endpoint = try directory.path.withCString { try result(easytier_ios_packet_tunnel_start($0)) as? [String: Any] }
                let initial = try snapshot()
                let settings = try initial.networkSettings()
                setTunnelNetworkSettings(settings) { error in
                    self.queue.async {
                        guard self.generation == current else { completionHandler(self.failure("启动已取消")); return }
                        if let error { self.cleanup(); completionHandler(error); return }
                        self.applied = initial
                        self.bridge?.start()
                        let timer = DispatchSource.makeTimerSource(queue: self.queue)
                        timer.schedule(deadline: .now() + .milliseconds(500), repeating: .milliseconds(500))
                        timer.setEventHandler { [weak self] in self?.refreshSettings() }
                        self.timer = timer; timer.resume()
                        self.logger.info("EasyTier Packet Tunnel 已启动")
                        completionHandler(nil)
                    }
                }
            } catch { cleanup(); completionHandler(error) }
        }
    }

    private func refreshSettings() {
        guard !applying else { return }
        do {
            let next = try snapshot()
            guard next != applied else { return }
            let settings = try next.networkSettings()
            let current = generation
            applying = true
            setTunnelNetworkSettings(settings) { error in
                self.queue.async {
                    guard current == self.generation else { return }
                    self.applying = false
                    if let error { self.cancelTunnelWithError(error) } else { self.applied = next }
                }
            }
        } catch { logger.error("更新隧道设置失败：\(error.localizedDescription, privacy: .public)"); cancelTunnelWithError(error) }
    }

    private func cleanup() {
        generation &+= 1
        timer?.cancel(); timer = nil
        bridge?.stopReading()
        _ = easytier_ios_packet_tunnel_stop()
        // 核心停止并释放所有网卡任务后才关闭描述符。
        bridge = nil; endpoint = nil; applied = nil; applying = false
    }

    override func stopTunnel(with reason: NEProviderStopReason, completionHandler: @escaping () -> Void) {
        queue.async { self.cleanup(); completionHandler() }
    }

    override func wake() { queue.async { self.refreshSettings() } }

    override func handleAppMessage(_ messageData: Data, completionHandler: ((Data?) -> Void)?) {
        queue.async { [self] in
            do {
                guard let message = try JSONSerialization.jsonObject(with: messageData) as? [String: Any] else { throw failure("无效的管理请求") }
                let value: Any
                switch message["command"] as? String {
                case "endpoint":
                    guard let endpoint else { throw failure("核心尚未启动") }
                    value = endpoint
                case "web":
                    let url = message["url"] as? String ?? ""
                    value = try url.withCString { try result(easytier_ios_packet_tunnel_web($0)) }
                case "webStatus": value = try result(easytier_ios_packet_tunnel_web(nil))
                default: throw failure("未知的管理请求")
                }
                completionHandler?(try JSONSerialization.data(withJSONObject: ["ok": true, "value": value]))
            } catch { completionHandler?(try? JSONSerialization.data(withJSONObject: ["ok": false, "error": error.localizedDescription])) }
        }
    }

    private func failure(_ message: String) -> NSError {
        NSError(domain: "EasyTier.PacketTunnel", code: 1, userInfo: [NSLocalizedDescriptionKey: message])
    }
}
