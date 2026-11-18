# iOS Packet Tunnel 客户端

此工程复用 EasyTier GUI，系统 VPN 授权、配置保存、连接和停止流程参考同一工作区
VPN 项目的 iOS 实现。数据面使用 EasyTier Rust 核心，未引入 Libbox、sing-box 或
HiLinka 的规则集。每个 Swift 类独立放置于一个文件。

## 进程与数据流

- App 负责界面、用户配置和 VPN 授权，核心及配置服务器客户端运行在 Packet Tunnel 扩展。
- App 通过系统 `sendProviderMessage` 获取会话端口和随机密钥，然后连接回环管理 RPC。
  服务绑定 `127.0.0.1`，先验证 32 字节随机密钥，再注册管理服务；握手有超时，
  最多保留 8 个管理连接，密钥不写入日志或描述文件。
- `PacketFlowBridge` 使用公开的 `readPackets` / `writePackets` 接口，通过有界的
  非阻塞 Unix 数据报套接字向核心传递 IP 包，不通过 KVC 访问系统内部描述符。
  数据面不经过 WebView，也不跨 App / 扩展进程传递裸文件描述符。
- 核心安装网卡有完成确认。停止时先停止 packetFlow，再停止核心及管理任务，
  最后关闭套接字。扩展读取真实运行地址，按地址、DNS 和 MTU 变化重新安装核心网卡。
- 扩展每 500 毫秒检查实际地址和路由；设置未变化时不调用系统配置接口。
  自动路由包含组网网段及远端子网代理；显式手工路由优先，配置出口节点才加入 IPv4 默认路由。
  空闲状态不接管默认路由、不修改 DNS，但保留 VPN 扩展及其管理连接。
- 当前只允许一个启用 TUN 的网络；其他 `no_tun` 网络可以共存。
- 活跃网络配置和配置服务器地址保存于扩展沙箱的私有文件；重启扩展时恢复。
  App 重开后读取扩展状态，不重复启动已有实例。

## 构建

需要 macOS、完整 Xcode、Node.js、pnpm 9、XcodeGen，以及支持 Network Extension
的 Apple 开发团队和签名配置。首次在仓库根目录运行：

```sh
rustup target add aarch64-apple-ios
pnpm install --frozen-lockfile
```

在 `easytier-gui` 目录运行：

```sh
export APPLE_DEVELOPMENT_TEAM="你的 Apple 开发团队 ID"
pnpm build:ios --ci
```

入口显式启用 `custom-protocol`，将前端资源嵌入包内，不访问 Mac 开发服务器。
Xcode 编译前通过 `script/build-ios-packet-tunnel.sh` 生成扩展专用的优化 Rust 静态库，
扩展不会链接 Tauri UI。核心使用两个 Tokio 工作线程。
产物位于 `easytier-gui/src-tauri/gen/apple/build/arm64/`。

工程配置为 `src-tauri/gen/apple/project.yml`，修改后运行：

```sh
xcodegen generate --spec src-tauri/gen/apple/project.yml
```

App ID 为 `com.yingnierxiao.easytier`，扩展 ID 为
`com.yingnierxiao.easytier.PacketTunnel`。两者都需要
`com.apple.developer.networking.networkextension = [packet-tunnel-provider]` 权限，
不能复用 HiLinka 的 Bundle ID 或通配符签名描述文件。

## 验证与安装

```sh
cargo check -p easytier-gui -p easytier-ios --target aarch64-apple-ios
cargo test -p easytier-ios --lib packet_tunnel_runtime::tests -- --test-threads=1
xcrun swiftc src-tauri/gen/apple/PacketTunnel/TunnelNetwork.swift \
  src-tauri/gen/apple/PacketTunnel/TunnelSnapshot.swift \
  ../script/tests/ios-tunnel-settings.swift -o /tmp/easytier-ios-settings-tests
/tmp/easytier-ios-settings-tests
```

上面的 Swift 路由测试从 `easytier-gui` 运行，Cargo 命令可从仓库根目录运行。
设备需要信任 Mac 并开启开发者模式。通过 MobileInstallation 安装开发签名 IPA：

```sh
pymobiledevice3 apps install --udid "$IOS_DEVICE_ID" --developer "$IOS_IPA_PATH"
xcrun devicectl device process launch --device "$IOS_DEVICE_ID" com.yingnierxiao.easytier
```

若开发镜像挂载失败，查看：

```sh
xcrun devicectl device info ddiServices --device "$IOS_DEVICE_ID" --verbose
```

`kAMAuthInstallErrorHTTPUnauthorized` 表示 Apple 镜像个性化请求返回 HTTP 401。
这会阻断自动启动和调试，不能将构建或安装成功当成真机 VPN 数据流验证。
真实验证应覆盖 VPN 授权、连接其他 EasyTier 节点、访问远端子网、DNS、锁屏后台、
Wi-Fi 切换、停止与重新连接；没有完成的项目应在交付时单独说明。
