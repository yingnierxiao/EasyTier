import SwiftRs
import Tauri
import Foundation

final class VpnservicePlugin: Plugin {
    private var controller: VpnTunnelController?

    @MainActor private func tunnel() -> VpnTunnelController {
        if let controller { return controller }
        let controller = VpnTunnelController()
        self.controller = controller
        return controller
    }

    @objc public func connectCore(_ invoke: Invoke) {
        Task { @MainActor in
            do { invoke.resolve(try await tunnel().connectCore()) }
            catch { invoke.reject(error.localizedDescription) }
        }
    }

    @objc public func coreCommand(_ invoke: Invoke) throws {
        let args = try invoke.parseArgs(CoreCommandArgs.self)
        Task { @MainActor in
            do {
                var message: [String: Any] = ["command": args.command]
                if let url = args.url { message["url"] = url }
                invoke.resolve(try await tunnel().request(message))
            } catch { invoke.reject(error.localizedDescription) }
        }
    }

    @objc public func ping(_ invoke: Invoke) throws {
        let args = try invoke.parseArgs(PingArgs.self)
        invoke.resolve(["value": args.value ?? ""])
    }

    @objc public func prepare_vpn(_ invoke: Invoke) { connectCore(invoke) }
    @objc public func start_vpn(_ invoke: Invoke) { connectCore(invoke) }
    @objc public func stop_vpn(_ invoke: Invoke) {
        Task { @MainActor in tunnel().stop(); invoke.resolve() }
    }
    @objc public func get_vpn_status(_ invoke: Invoke) {
        Task { @MainActor in invoke.resolve(["running": tunnel().running]) }
    }
    @objc public func consume_vpn_tile_action(_ invoke: Invoke) { invoke.resolve([:]) }
}

@_cdecl("init_plugin_vpnservice")
func initPlugin() -> Plugin { VpnservicePlugin() }
