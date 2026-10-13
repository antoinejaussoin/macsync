import Darwin
import Foundation

let homebrewRsyncCandidates = ["/opt/homebrew/bin/rsync", "/usr/local/bin/rsync"]
let systemRsync = "/usr/bin/rsync"

struct RsyncInfo: Equatable {
    var path: String
    var version: String
}

enum RsyncStatus: Equatable {
    case found(RsyncInfo)
    case missing(String)

    var info: RsyncInfo? {
        if case .found(let info) = self { return info }
        return nil
    }

    var message: String? {
        if case .missing(let message) = self { return message }
        return nil
    }
}

/// The copy shipped in `MacSync.app/Contents/MacOS`, when this process is that app.
func bundledRsyncPath() -> String? {
    let executable = Bundle.main.executableURL?.path ?? CommandLine.arguments.first
    guard let executable, !executable.isEmpty else { return nil }
    let path = URL(fileURLWithPath: executable).deletingLastPathComponent().appendingPathComponent("rsync").path
    return FileManager.default.isExecutableFile(atPath: path) ? path : nil
}

func rsyncSearchPaths(overridePath: String?, bundledPath: String?) -> [String] {
    if let overridePath, !overridePath.isEmpty { return [overridePath] }
    var paths: [String] = []
    if let bundledPath, !bundledPath.isEmpty { paths.append(bundledPath) }
    paths.append(contentsOf: homebrewRsyncCandidates)
    return paths
}

func locateRsync(overridePath: String?) -> RsyncStatus {
    let candidates = rsyncSearchPaths(overridePath: overridePath, bundledPath: bundledRsyncPath())
    var problems: [String] = []
    for path in candidates {
        if path == systemRsync {
            problems.append("\(systemRsync) is Apple's openrsync, which is not supported")
            continue
        }
        var isDir: ObjCBool = false
        guard FileManager.default.fileExists(atPath: path, isDirectory: &isDir), !isDir.boolValue else { continue }
        let process = Process()
        process.executableURL = URL(fileURLWithPath: path)
        process.arguments = ["--version"]
        let output = Pipe()
        process.standardOutput = output
        process.standardError = Pipe()
        do {
            try process.run()
            process.waitUntilExit()
        } catch {
            return .missing("Could not run \(path): \(error)")
        }
        let data = output.fileHandleForReading.readDataToEndOfFile()
        let text = String(data: data, encoding: .utf8) ?? ""
        switch parseRsyncVersion(text) {
        case .success(let version):
            return .found(RsyncInfo(path: path, version: version))
        case .failure(let error):
            problems.append("\(path): \(error.message)")
        }
    }
    let hint = "MacSync.app includes rsync. An unpackaged build can use Homebrew instead: brew install rsync"
    if problems.isEmpty {
        return .missing("rsync was not found. \(hint)")
    }
    return .missing("\(problems.joined(separator: "; ")). \(hint)")
}

func parseRsyncVersion(_ output: String) -> Result<String, AppError> {
    let first = output.split(whereSeparator: \.isNewline).first.map(String.init) ?? ""
    let tokens = first.split(whereSeparator: \.isWhitespace).map(String.init)
    guard tokens.count >= 3, tokens[0] == "rsync", tokens[1] == "version" else {
        return .failure(AppError("not GNU rsync (\"\(first.trimmingCharacters(in: .whitespaces))\")"))
    }
    let version = tokens[2].hasPrefix("v") ? String(tokens[2].dropFirst()) : tokens[2]
    let parts = version.split(separator: ".").map { UInt32($0) ?? 0 }
    let major = parts.first ?? 0
    let minor = parts.count > 1 ? parts[1] : 0
    if major != 3 || minor < 1 {
        return .failure(AppError("rsync \(version) is too old, 3.1 or newer is required"))
    }
    return .success(version)
}

struct BackupArguments {
    var source: String
    var target: String
    var linkDest: String?
    var excludes: [String]
    var readOnly: Bool
}

func backupArguments(_ arguments: BackupArguments) -> [String] {
    var args = ["-aHAX", "--delete", "--delete-excluded", "--info=stats2,progress2"]
    if arguments.readOnly { args.append("--chmod=Fa-w") }
    appendExcludes(&args, arguments.excludes)
    if let linkDest = arguments.linkDest {
        args.append("--link-dest=\(linkDest)")
    }
    args.append(directoryArgument(arguments.source))
    args.append(directoryArgument(arguments.target))
    return args
}

func integrityArguments(source: String, snapshot: String, excludes: [String], readOnly: Bool) -> [String] {
    var args = ["-aHAX", "--dry-run", "--checksum", "--delete", "--out-format=%i|%n"]
    if readOnly { args.append("--chmod=Fa-w") }
    appendExcludes(&args, excludes)
    args.append(directoryArgument(source))
    args.append(directoryArgument(snapshot))
    return args
}

private func appendExcludes(_ args: inout [String], _ excludes: [String]) {
    for pattern in excludes.map({ $0.trimmingCharacters(in: .whitespacesAndNewlines) }) where !pattern.isEmpty {
        args.append("--exclude=\(pattern)")
    }
}

private func directoryArgument(_ path: String) -> String {
    path.hasSuffix("/") ? path : path + "/"
}

struct TransferProgress: Equatable {
    var bytes: UInt64
    var percent: Int
    var rate: String
    var eta: String
    var filesTransferred: UInt64
    /// `ir-chk` means rsync is still discovering files, so `percent` is not the whole job.
    var scanning: Bool
    var filesChecked: Int?
    var filesTotal: Int?
}

func parseProgress(_ line: String) -> TransferProgress? {
    let tokens = line.split(whereSeparator: \.isWhitespace).map(String.init)
    guard tokens.count >= 4, tokens[1].hasSuffix("%") else { return nil }
    let bytesText = tokens[0].replacingOccurrences(of: ",", with: "")
    guard let bytes = UInt64(bytesText), let percent = Int(tokens[1].dropLast()) else { return nil }
    var filesTransferred: UInt64 = 0
    // Until `to-chk` appears, rsync is still discovering files and its percentage
    // is not the size of the whole backup.
    var scanning = true
    var filesChecked: Int?
    var filesTotal: Int?
    for token in tokens {
        if token.hasPrefix("(xfr#") || token.hasPrefix("xfr#") {
            let raw = token.hasPrefix("(xfr#") ? token.dropFirst(5) : token.dropFirst(4)
            let number = raw.trimmingCharacters(in: CharacterSet(charactersIn: ",)"))
            filesTransferred = UInt64(number) ?? 0
        }
        for prefix in ["ir-chk=", "to-chk="] where token.contains(prefix) {
            guard let range = token.range(of: prefix) else { continue }
            let rest = token[range.upperBound...].trimmingCharacters(in: CharacterSet(charactersIn: ")"))
            let parts = rest.split(separator: "/")
            guard parts.count == 2, let remaining = Int(parts[0]), let total = Int(parts[1]) else { continue }
            scanning = prefix == "ir-chk="
            filesTotal = total
            filesChecked = scanning ? nil : max(0, total - remaining)
        }
    }
    return TransferProgress(
        bytes: bytes,
        percent: percent,
        rate: tokens[2],
        eta: tokens[3],
        filesTransferred: filesTransferred,
        scanning: scanning,
        filesChecked: filesChecked,
        filesTotal: filesTotal
    )
}

final class CancelHandle: @unchecked Sendable {
    private let lock = NSLock()
    private var pgid: Int32 = 0
    private var cancelled = false

    func cancel() {
        lock.lock()
        cancelled = true
        let group = pgid
        lock.unlock()
        if group > 0 {
            kill(-group, SIGTERM)
        }
    }

    func setGroup(_ group: Int32) {
        lock.lock()
        pgid = group
        let shouldKill = cancelled && group > 0
        lock.unlock()
        if shouldKill { kill(-group, SIGTERM) }
    }

    func clearGroup() {
        lock.lock()
        pgid = 0
        lock.unlock()
    }

    var isCancelled: Bool {
        lock.lock()
        defer { lock.unlock() }
        return cancelled
    }
}

struct RunOutput {
    var exitCode: Int32
    var stdoutLines: [String]
    var stderrTail: [String]
}

func runRsync(
    path: String,
    arguments: [String],
    log: LogFile,
    cancel: CancelHandle,
    keepStdout: Bool,
    onProgress: (TransferProgress) -> Void
) throws -> RunOutput {
    let child = try spawnProcess(path: path, arguments: arguments)
    cancel.setGroup(child.pid)
    if cancel.isCancelled { cancel.cancel() }

    let stderrTail = LockedLines()
    let group = DispatchGroup()
    group.enter()
    DispatchQueue.global(qos: .utility).async {
        defer { group.leave() }
        let text = readAll(fd: child.stderr)
        for line in textLines(text) where !line.trimmingCharacters(in: .whitespaces).isEmpty {
            log.line(line)
            stderrTail.append(line, limit: 20)
        }
        Darwin.close(child.stderr)
    }

    var stdoutLines: [String] = []
    readChunks(fd: child.stdout) { raw in
        let line = String(decoding: raw, as: UTF8.self)
        if line.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty { return }
        if let progress = parseProgress(line) {
            onProgress(progress)
        } else {
            log.line(line)
            if keepStdout { stdoutLines.append(line) }
        }
    }
    Darwin.close(child.stdout)

    let status = waitFor(child.pid)
    cancel.clearGroup()
    group.wait()
    return RunOutput(exitCode: status, stdoutLines: stdoutLines, stderrTail: stderrTail.values)
}

func rsyncExitMeaning(_ code: Int32) -> String {
    switch code {
    case 0: return "success"
    case 1: return "syntax or usage error"
    case 2: return "protocol incompatibility"
    case 3: return "errors selecting input/output files or folders"
    case 11: return "error in file I/O (disk full?)"
    case 12: return "error in rsync data stream"
    case 20: return "stopped"
    case 23: return "some files could not be copied (permissions? Full Disk Access?)"
    case 24: return "some source files vanished during the copy"
    case 30: return "timeout"
    default: return "rsync error"
    }
}

func displayArguments(_ arguments: [String]) -> String {
    arguments.map { argument in
        argument.contains(" ") ? "'\(argument)'" : argument
    }.joined(separator: " ")
}

struct Spawned {
    var pid: pid_t
    var stdout: Int32
    var stderr: Int32
}

func spawnProcess(path: String, arguments: [String]) throws -> Spawned {
    var outPipe: [Int32] = [0, 0]
    var errPipe: [Int32] = [0, 0]
    guard Darwin.pipe(&outPipe) == 0, Darwin.pipe(&errPipe) == 0 else {
        throw AppError("Could not start rsync: \(String(cString: strerror(errno)))")
    }
    for fd in [outPipe[0], outPipe[1], errPipe[0], errPipe[1]] {
        _ = Darwin.fcntl(fd, F_SETFD, FD_CLOEXEC)
    }
    var actions: posix_spawn_file_actions_t?
    posix_spawn_file_actions_init(&actions)
    defer { posix_spawn_file_actions_destroy(&actions) }
    let devnull = Darwin.open("/dev/null", O_RDONLY)
    if devnull >= 0 {
        posix_spawn_file_actions_adddup2(&actions, devnull, STDIN_FILENO)
        posix_spawn_file_actions_addclose(&actions, devnull)
    }
    posix_spawn_file_actions_adddup2(&actions, outPipe[1], STDOUT_FILENO)
    posix_spawn_file_actions_adddup2(&actions, errPipe[1], STDERR_FILENO)
    posix_spawn_file_actions_addclose(&actions, outPipe[0])
    posix_spawn_file_actions_addclose(&actions, outPipe[1])
    posix_spawn_file_actions_addclose(&actions, errPipe[0])
    posix_spawn_file_actions_addclose(&actions, errPipe[1])

    var attr: posix_spawnattr_t?
    posix_spawnattr_init(&attr)
    defer { posix_spawnattr_destroy(&attr) }
    posix_spawnattr_setflags(&attr, Int16(POSIX_SPAWN_SETPGROUP))
    posix_spawnattr_setpgroup(&attr, 0)

    let argvStrings = [path] + arguments
    let cArgs: [UnsafeMutablePointer<CChar>?] = argvStrings.map { strdup($0) }
    defer { cArgs.forEach { free($0) } }
    var cArgv: [UnsafeMutablePointer<CChar>?] = cArgs
    cArgv.append(nil)

    var pid: pid_t = 0
    let rc: Int32 = cArgv.withUnsafeMutableBufferPointer { buffer in
        posix_spawn(&pid, path, &actions, &attr, buffer.baseAddress, environ)
    }
    if devnull >= 0 { Darwin.close(devnull) }
    Darwin.close(outPipe[1])
    Darwin.close(errPipe[1])
    if rc != 0 {
        Darwin.close(outPipe[0])
        Darwin.close(errPipe[0])
        throw AppError("Could not start rsync: \(String(cString: strerror(rc)))")
    }
    return Spawned(pid: pid, stdout: outPipe[0], stderr: errPipe[0])
}

private func readChunks(fd: Int32, onLine: ([UInt8]) -> Void) {
    var buffer = [UInt8](repeating: 0, count: 16 * 1024)
    var pending: [UInt8] = []
    while true {
        let count = Darwin.read(fd, &buffer, buffer.count)
        if count == 0 { break }
        if count < 0 {
            if errno == EINTR { continue }
            break
        }
        for byte in buffer.prefix(count) {
            if byte == 10 || byte == 13 {
                onLine(pending)
                pending.removeAll(keepingCapacity: true)
            } else {
                pending.append(byte)
            }
        }
    }
    onLine(pending)
}

private func readAll(fd: Int32) -> String {
    var data = Data()
    var buffer = [UInt8](repeating: 0, count: 16 * 1024)
    while true {
        let count = Darwin.read(fd, &buffer, buffer.count)
        if count == 0 { break }
        if count < 0 {
            if errno == EINTR { continue }
            break
        }
        data.append(buffer, count: count)
    }
    return String(data: data, encoding: .utf8) ?? ""
}

private func waitFor(_ pid: pid_t) -> Int32 {
    while true {
        var status: Int32 = 0
        let result = Darwin.waitpid(pid, &status, 0)
        if result == pid {
            if status & 0x7f == 0 { return (status >> 8) & 0xff }
            return -1
        }
        if result < 0 && errno == EINTR { continue }
        return -1
    }
}

private final class LockedLines: @unchecked Sendable {
    private let lock = NSLock()
    private var lines: [String] = []
    func append(_ line: String, limit: Int) {
        lock.lock()
        lines.append(line)
        if lines.count > limit { lines.removeFirst(lines.count - limit) }
        lock.unlock()
    }

    var values: [String] {
        lock.lock()
        defer { lock.unlock() }
        return lines
    }
}

final class LogFile {
    private let fd: Int32
    private let lock = NSLock()

    init(pairID: String) throws {
        let url = Paths.logFile(pairID: pairID)
        let directory = url.deletingLastPathComponent()
        do {
            try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        } catch {
            throw AppError("Could not create \(directory.path): \(error)")
        }
        fd = Darwin.open(url.path, O_CREAT | O_APPEND | O_WRONLY, 0o644)
        if fd < 0 {
            throw AppError("Could not open log \(url.path): \(String(cString: strerror(errno)))")
        }
        line("\n=== \(SelfDate.logHeader.string(from: Date())) ===")
    }

    func line(_ text: String) {
        lock.lock()
        defer { lock.unlock() }
        let payload = text.hasSuffix("\n") ? text : text + "\n"
        _ = payload.withCString { Darwin.write(fd, $0, strlen($0)) }
    }

    deinit { Darwin.close(fd) }
}
