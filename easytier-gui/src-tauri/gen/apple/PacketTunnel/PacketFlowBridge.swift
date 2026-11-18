import Darwin
import Foundation
import NetworkExtension

/// 通过公开 packetFlow 接口与核心交换数据包；文件描述符始终留在扩展进程。
final class PacketFlowBridge {
    let coreFD: Int32
    private let hostFD: Int32
    private let flow: NEPacketTunnelFlow
    private let queue = DispatchQueue(label: "easytier.packet-flow")
    private var source: DispatchSourceRead?
    private var active = false
    private let buffer = UnsafeMutableRawPointer.allocate(byteCount: 65540, alignment: 8)
    private(set) var droppedPackets: UInt64 = 0

    init(flow: NEPacketTunnelFlow) throws {
        var descriptors: [Int32] = [-1, -1]
        guard socketpair(AF_UNIX, SOCK_DGRAM, 0, &descriptors) == 0 else {
            throw POSIXError(POSIXErrorCode(rawValue: errno) ?? .EIO)
        }
        coreFD = descriptors[0]
        hostFD = descriptors[1]
        self.flow = flow
        for fd in descriptors {
            var capacity: Int32 = 131072
            guard setsockopt(fd, SOL_SOCKET, SO_SNDBUF, &capacity, socklen_t(MemoryLayout.size(ofValue: capacity))) == 0,
                  setsockopt(fd, SOL_SOCKET, SO_RCVBUF, &capacity, socklen_t(MemoryLayout.size(ofValue: capacity))) == 0,
                  fcntl(fd, F_SETFD, FD_CLOEXEC) == 0 else {
                close(descriptors[0]); close(descriptors[1])
                throw POSIXError(.EIO)
            }
            guard fcntl(fd, F_SETFL, fcntl(fd, F_GETFL) | O_NONBLOCK) == 0 else {
                close(descriptors[0]); close(descriptors[1])
                throw POSIXError(.EIO)
            }
        }
    }

    func start() {
        queue.async { [self] in
            active = true
            let source = DispatchSource.makeReadSource(fileDescriptor: hostFD, queue: queue)
            source.setEventHandler { [weak self] in self?.drainCorePackets() }
            self.source = source
            source.resume()
            readSystemPackets()
        }
    }

    private func readSystemPackets() {
        guard active else { return }
        flow.readPackets { [weak self] packets, protocols in
            guard let self else { return }
            self.queue.async {
                guard self.active else { return }
                for (packet, proto) in zip(packets, protocols) {
                    guard !packet.isEmpty, packet.count <= 65535 else { continue }
                    var family = proto.uint32Value.bigEndian
                    // 分散写入协议头和原始包，避免为每个入站包重新拼接 Data。
                    let sent = withUnsafeMutablePointer(to: &family) { header in
                        packet.withUnsafeBytes { body in
                            var vectors = [iovec(iov_base: header, iov_len: 4), iovec(iov_base: UnsafeMutableRawPointer(mutating: body.baseAddress), iov_len: body.count)]
                            return vectors.withUnsafeMutableBufferPointer { vectors in
                                var message = msghdr()
                                message.msg_iov = vectors.baseAddress
                                message.msg_iovlen = 2
                                return sendmsg(self.hostFD, &message, 0)
                            }
                        }
                    }
                    if sent != packet.count + 4 { self.droppedPackets &+= 1 }
                }
                self.readSystemPackets()
            }
        }
    }

    private func drainCorePackets() {
        guard active else { return }
        var packets: [Data] = []
        var protocols: [NSNumber] = []
        packets.reserveCapacity(32); protocols.reserveCapacity(32)
        for _ in 0..<32 {
            let count = recv(hostFD, buffer, 65540, 0)
            if count < 0 { break }
            guard count > 4 else { continue }
            let family = UInt32(bigEndian: buffer.load(as: UInt32.self))
            guard family == UInt32(AF_INET) || family == UInt32(AF_INET6) else { continue }
            packets.append(Data(bytes: buffer.advanced(by: 4), count: count - 4))
            protocols.append(NSNumber(value: family))
        }
        if !packets.isEmpty && !flow.writePackets(packets, withProtocols: protocols) {
            droppedPackets &+= UInt64(packets.count)
        }
    }

    func stopReading() {
        queue.sync { active = false; source?.cancel(); source = nil }
    }

    deinit {
        close(coreFD); close(hostFD)
        buffer.deallocate()
    }
}
