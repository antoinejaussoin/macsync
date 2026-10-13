import Darwin
import Foundation

/// Held for the life of the process. A second copy would run a second scheduler.
final class Instance {
    let lockFD: Int32

    private init(lockFD: Int32) {
        self.lockFD = lockFD
    }

    deinit { Darwin.close(lockFD) }

    /// Returns nil when another copy is already running. A copy started by the user
    /// (not at login) asks the running one to open its window before exiting.
    static func acquire(background: Bool, onShow: @escaping () -> Void) -> Instance? {
        try? FileManager.default.createDirectory(at: Paths.appSupport, withIntermediateDirectories: true)
        let fd = Darwin.open(Paths.lockFile.path, O_CREAT | O_RDWR, 0o644)
        if fd < 0 { return nil }
        if flock(fd, LOCK_EX | LOCK_NB) != 0 {
            Darwin.close(fd)
            if !background {
                signalRunningInstance(Paths.socketFile.path)
            }
            return nil
        }
        let socketPath = Paths.socketFile.path
        unlink(socketPath)
        if let server = bindShowSocket(socketPath) {
            Thread.detachNewThread {
                acceptShowRequests(server, onShow: onShow)
            }
        }
        return Instance(lockFD: fd)
    }
}

private func signalRunningInstance(_ path: String) {
    let fd = socket(AF_UNIX, SOCK_STREAM, 0)
    if fd < 0 { return }
    defer { Darwin.close(fd) }
    var address = sockaddr_un()
    guard fillUnixAddress(&address, path: path) else { return }
    let size = socklen_t(MemoryLayout<sockaddr_un>.size)
    let connected = withUnsafePointer(to: &address) {
        $0.withMemoryRebound(to: sockaddr.self, capacity: 1) { connect(fd, $0, size) }
    }
    if connected != 0 { return }
    _ = "show".withCString { Darwin.write(fd, $0, 4) }
}

private func bindShowSocket(_ path: String) -> Int32? {
    let fd = socket(AF_UNIX, SOCK_STREAM, 0)
    if fd < 0 { return nil }
    var address = sockaddr_un()
    guard fillUnixAddress(&address, path: path) else {
        Darwin.close(fd)
        return nil
    }
    let size = socklen_t(MemoryLayout<sockaddr_un>.size)
    let bound = withUnsafePointer(to: &address) {
        $0.withMemoryRebound(to: sockaddr.self, capacity: 1) { bind(fd, $0, size) }
    }
    if bound != 0 || listen(fd, 16) != 0 {
        Darwin.close(fd)
        return nil
    }
    return fd
}

private func fillUnixAddress(_ address: inout sockaddr_un, path: String) -> Bool {
    address.sun_family = sa_family_t(AF_UNIX)
    return path.withCString { source in
        withUnsafeMutablePointer(to: &address.sun_path.0) { destination in
            strlcpy(destination, source, 104) < 104
        }
    }
}

private func acceptShowRequests(_ fd: Int32, onShow: @escaping () -> Void) {
    while true {
        let client = accept(fd, nil, nil)
        if client < 0 { continue }
        var buffer = [UInt8](repeating: 0, count: 64)
        var data = Data()
        while data.count < 64 {
            let count = Darwin.read(client, &buffer, buffer.count)
            if count <= 0 { break }
            data.append(buffer, count: count)
        }
        Darwin.close(client)
        if String(data: data, encoding: .utf8) == "show" {
            onShow()
        }
    }
}
