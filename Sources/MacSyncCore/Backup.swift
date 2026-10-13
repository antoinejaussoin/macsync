import Darwin
import Foundation

let latestNameLink = "latest"
let inProgressPrefix = ".inprogress-"

func listSnapshots(dest: URL) throws -> [(String, SnapshotStamp)] {
    let entries = try FileManager.default.contentsOfDirectory(
        at: dest,
        includingPropertiesForKeys: [.isDirectoryKey, .isSymbolicLinkKey],
        options: []
    )
    var snapshots: [(String, SnapshotStamp)] = []
    for entry in entries {
        let name = entry.lastPathComponent
        guard let stamp = parseStamp(name) else { continue }
        let values = try entry.resourceValues(forKeys: [.isDirectoryKey, .isSymbolicLinkKey])
        if values.isSymbolicLink == true { continue }
        if values.isDirectory == true {
            snapshots.append((name, stamp))
        }
    }
    snapshots.sort { $0.1 > $1.1 }
    return snapshots
}

func latestSnapshotName(dest: URL) -> String? {
    let link = dest.appendingPathComponent(latestNameLink).path
    guard let target = try? FileManager.default.destinationOfSymbolicLink(atPath: link) else { return nil }
    let name = (target as NSString).lastPathComponent
    guard parseStamp(name) != nil else { return nil }
    var isDir: ObjCBool = false
    let folder = dest.appendingPathComponent(name).path
    guard FileManager.default.fileExists(atPath: folder, isDirectory: &isDir), isDir.boolValue else { return nil }
    return name
}

func setLatest(dest: URL, name: String) throws {
    let tmp = dest.appendingPathComponent(".latest.tmp")
    if fileExists(tmp.path) {
        try? FileManager.default.removeItem(at: tmp)
    }
    if symlink(name, tmp.path) != 0 {
        throw AppError(String(cString: strerror(errno)))
    }
    let latest = dest.appendingPathComponent(latestNameLink)
    if rename(tmp.path, latest.path) != 0 {
        throw AppError(String(cString: strerror(errno)))
    }
}

func makeReadOnly(_ directory: URL) throws {
    var failure: Error?
    walkDirectories(directory) { path in
        chmodDirectory(path) { mode in mode & ~0o222 }
    } onError: { error in
        failure = error
    }
    if let failure { throw failure }
}

func removeTree(_ directory: URL) throws {
    walkDirectories(directory) { path in
        chmodDirectory(path) { mode in mode | 0o700 }
    } onError: { _ in }
    do {
        try FileManager.default.removeItem(at: directory)
    } catch {
        throw error
    }
}

private func fileExists(_ path: String) -> Bool {
    var status = stat()
    return lstat(path, &status) == 0
}

private func chmodDirectory(_ path: String, _ transform: (mode_t) -> mode_t) {
    var status = stat()
    guard lstat(path, &status) == 0 else { return }
    guard (status.st_mode & S_IFMT) == S_IFDIR else { return }
    _ = chmod(path, transform(status.st_mode))
}

private func walkDirectories(_ root: URL, _ body: (String) -> Void, onError: (Error) -> Void) {
    body(root.path)
    let children: [URL]
    do {
        children = try FileManager.default.contentsOfDirectory(
            at: root,
            includingPropertiesForKeys: [.isDirectoryKey, .isSymbolicLinkKey],
            options: []
        )
    } catch {
        onError(error)
        return
    }
    for child in children {
        let values = try? child.resourceValues(forKeys: [.isDirectoryKey, .isSymbolicLinkKey])
        if values?.isSymbolicLink == true { continue }
        if values?.isDirectory == true {
            walkDirectories(child, body, onError: onError)
        }
    }
}

func pruneSnapshots(dest: URL, policy: Retention, log: LogFile) throws -> [String] {
    let snapshots: [(String, SnapshotStamp)]
    do {
        snapshots = try listSnapshots(dest: dest)
    } catch {
        throw AppError("Could not list snapshots: \(error)")
    }
    let latest = latestSnapshotName(dest: dest)
    var removed: [String] = []
    for decision in retentionPlan(snapshots: snapshots, policy: policy, latest: latest).reversed() {
        if decision.keep { continue }
        log.line("Retention: removing \(decision.name)")
        do {
            try removeTree(dest.appendingPathComponent(decision.name))
        } catch {
            throw AppError("Could not remove \(decision.name): \(error)")
        }
        removed.append(decision.name)
    }
    return removed
}

struct FsInfo: Equatable {
    var fsType: String
    var freeBytes: UInt64
}

func filesystemInfo(path: String) -> FsInfo? {
    var status = statfs()
    guard path.withCString({ statfs($0, &status) }) == 0 else { return nil }
    let type = withUnsafePointer(to: &status.f_fstypename) { pointer -> String in
        pointer.withMemoryRebound(to: CChar.self, capacity: Int(MFSTYPENAMELEN)) {
            String(cString: $0)
        }
    }
    let (bytes, overflow) = status.f_bavail.multipliedReportingOverflow(by: UInt64(status.f_bsize))
    return FsInfo(fsType: type, freeBytes: overflow ? UInt64.max : bytes)
}

func filesystemSupportsHardLinks(_ fsType: String) -> Bool {
    !["msdos", "exfat", "ntfs"].contains(fsType)
}

enum BackupError: Error, Equatable, CustomStringConvertible {
    case destinationMissing(String)
    case cancelled
    case failed(String)

    var description: String {
        switch self {
        case .destinationMissing(let message), .failed(let message): return message
        case .cancelled: return "Cancelled"
        }
    }
}

struct BackupResult: Equatable {
    var snapshot: String
    var pruned: [String]
    var summary: String
    var warning: String?
}

func runBackup(
    pair: Pair,
    rsync: RsyncInfo,
    log: LogFile,
    cancel: CancelHandle,
    onProgress: (TransferProgress) -> Void
) -> Result<BackupResult, BackupError> {
    let source: String
    let destPath: String
    switch checkPairPaths(source: pair.source, destination: pair.destination) {
    case .success(let paths):
        source = paths.0
        destPath = paths.1
    case .failure(let error):
        switch error {
        case .destinationMissing:
            return .failure(.destinationMissing(error.description))
        default:
            return .failure(.failed(error.description))
        }
    }
    let dest = URL(fileURLWithPath: destPath, isDirectory: true)
    if let info = filesystemInfo(path: destPath), !filesystemSupportsHardLinks(info.fsType) {
        return .failure(.failed(
            "The destination drive is formatted as \(info.fsType), which has no hard links. Use an APFS or Mac OS Extended drive."
        ))
    }
    do {
        let entries = try FileManager.default.contentsOfDirectory(atPath: source)
        if entries.isEmpty {
            return .failure(.failed("Source folder is empty; not creating an empty snapshot"))
        }
    } catch {
        return .failure(.failed(
            "Cannot read the source folder (\(error)). Grant MacSync Full Disk Access in System Settings > Privacy & Security."
        ))
    }

    cleanupInProgress(dest: dest, log: log)

    var stamp = formatStamp(Date())
    while fileExists(dest.appendingPathComponent(stamp).path) {
        Thread.sleep(forTimeInterval: 1.1)
        stamp = formatStamp(Date())
    }
    let work = dest.appendingPathComponent("\(inProgressPrefix)\(stamp)")
    let linkName = latestSnapshotName(dest: dest) ?? (try? listSnapshots(dest: dest))?.first?.0
    let linkDest = linkName.map { dest.appendingPathComponent($0).path }
    let args = backupArguments(BackupArguments(
        source: source,
        target: work.path,
        linkDest: linkDest,
        excludes: pair.excludes,
        readOnly: pair.readOnly
    ))
    log.line("Snapshot \(stamp) of \(source) (linking against \(linkName ?? "nothing: first snapshot"))")
    log.line("\(rsync.path) \(displayArguments(args))")

    let output: RunOutput
    do {
        output = try runRsync(path: rsync.path, arguments: args, log: log, cancel: cancel, keepStdout: true, onProgress: onProgress)
    } catch {
        return .failure(.failed(error.localizedDescription))
    }
    if cancel.isCancelled {
        log.line("Cancelled; the incomplete snapshot is removed on the next run")
        return .failure(.cancelled)
    }
    var warning: String?
    switch output.exitCode {
    case 0:
        break
    case 24:
        warning = rsyncExitMeaning(24)
    default:
        let detail = output.stderrTail.last ?? ""
        log.line("rsync failed with exit code \(output.exitCode); latest still points at the previous snapshot")
        let suffix = detail.isEmpty ? "" : " (\(detail))"
        return .failure(.failed("rsync exit \(output.exitCode): \(rsyncExitMeaning(output.exitCode))\(suffix)"))
    }

    let finalURL = dest.appendingPathComponent(stamp)
    do {
        try FileManager.default.moveItem(at: work, to: finalURL)
    } catch {
        return .failure(.failed("Could not finalize snapshot: \(error)"))
    }
    do {
        try setLatest(dest: dest, name: stamp)
    } catch {
        return .failure(.failed("Could not update latest: \(error)"))
    }
    let pruned: [String]
    do {
        pruned = try pruneSnapshots(dest: dest, policy: pair.retention, log: log)
    } catch {
        return .failure(.failed(error.localizedDescription))
    }
    if pair.readOnly {
        do {
            try makeReadOnly(finalURL)
        } catch {
            log.line("Could not make snapshot read-only: \(error)")
        }
    }
    let summary = summarizeStats(output.stdoutLines)
    log.line("Snapshot \(stamp) complete. \(summary). Removed \(pruned.count) old snapshot(s).")
    return .success(BackupResult(snapshot: stamp, pruned: pruned, summary: summary, warning: warning))
}

private func cleanupInProgress(dest: URL, log: LogFile) {
    guard let entries = try? FileManager.default.contentsOfDirectory(
        at: dest,
        includingPropertiesForKeys: [.isDirectoryKey, .isSymbolicLinkKey],
        options: []
    ) else { return }
    for entry in entries {
        let name = entry.lastPathComponent
        let values = try? entry.resourceValues(forKeys: [.isDirectoryKey, .isSymbolicLinkKey])
        guard name.hasPrefix(inProgressPrefix), values?.isSymbolicLink != true, values?.isDirectory == true else { continue }
        log.line("Removing incomplete snapshot \(name)")
        do {
            try removeTree(entry)
        } catch {
            log.line("  could not remove: \(error)")
        }
    }
}

func summarizeStats(_ lines: [String]) -> String {
    func value(_ prefix: String) -> UInt64? {
        for line in lines {
            let trimmed = line.trimmingCharacters(in: .whitespaces)
            guard trimmed.hasPrefix(prefix) else { continue }
            let rest = trimmed.dropFirst(prefix.count).trimmingCharacters(in: .whitespaces)
            guard let token = rest.split(whereSeparator: \.isWhitespace).first else { continue }
            return UInt64(token.replacingOccurrences(of: ",", with: ""))
        }
        return nil
    }
    let copied = value("Number of regular files transferred:")
    let total = value("Number of files:")
    let bytes = value("Total transferred file size:")
    switch (copied, total, bytes) {
    case let (copied?, total?, bytes?):
        return "\(copied) files copied (\(formatBytes(bytes))) of \(total)"
    case let (copied?, _, _):
        return "\(copied) files copied"
    default:
        return "Finished"
    }
}

func suggestedDestination(picked: String, source: String) -> String {
    let pickedURL = URL(fileURLWithPath: picked, isDirectory: true)
    let hasSnapshots = fileExists(pickedURL.appendingPathComponent(latestNameLink).path)
        || ((try? listSnapshots(dest: pickedURL))?.isEmpty == false)
    let isEmpty = (try? FileManager.default.contentsOfDirectory(atPath: picked).isEmpty) == true
    if hasSnapshots || isEmpty { return picked }
    let folder = URL(fileURLWithPath: source).lastPathComponent
    let name = folder.isEmpty ? "MacSync snapshots" : "\(folder) snapshots"
    return pickedURL.appendingPathComponent(name).path
}
