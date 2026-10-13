import Foundation

func measureStats(pair: Pair) -> Stats {
    var stats = Stats.empty
    stats.measured = Date()
    var errors: [String] = []
    var sourceIsDir: ObjCBool = false
    if FileManager.default.fileExists(atPath: pair.source, isDirectory: &sourceIsDir), sourceIsDir.boolValue {
        switch duTotal([pair.source]) {
        case .success(let bytes): stats.sourceBytes = bytes
        case .failure(let error): errors.append("source: \(error.message)")
        }
    } else {
        errors.append("source folder not found")
    }
    var destIsDir: ObjCBool = false
    if FileManager.default.fileExists(atPath: pair.destination, isDirectory: &destIsDir), destIsDir.boolValue {
        if let info = filesystemInfo(path: pair.destination) {
            stats.freeBytes = info.freeBytes
            stats.filesystem = info.fsType
        }
        do {
            let snapshots = try listSnapshots(dest: URL(fileURLWithPath: pair.destination, isDirectory: true))
            stats.snapshotCount = snapshots.count
            stats.newest = snapshots.first?.0
            stats.oldest = snapshots.last?.0
            let dirs = snapshots.map { URL(fileURLWithPath: pair.destination).appendingPathComponent($0.0).path }
            switch duTotal(dirs) {
            case .success(let bytes): stats.destinationBytes = bytes
            case .failure(let error): errors.append("destination: \(error.message)")
            }
        } catch {
            errors.append("destination: \(error)")
        }
    } else {
        errors.append("destination not available")
    }
    if !errors.isEmpty { stats.error = errors.joined(separator: "; ") }
    return stats
}

func duTotal(_ paths: [String]) -> Result<UInt64, AppError> {
    if paths.isEmpty { return .success(0) }
    let process = Process()
    process.executableURL = URL(fileURLWithPath: "/usr/bin/du")
    process.arguments = ["-skxc"] + paths
    let stdout = Pipe()
    let stderr = Pipe()
    process.standardOutput = stdout
    process.standardError = stderr
    do {
        try process.run()
        process.waitUntilExit()
    } catch {
        return .failure(AppError(error.localizedDescription))
    }
    let text = String(data: stdout.fileHandleForReading.readDataToEndOfFile(), encoding: .utf8) ?? ""
    let total = text.split(whereSeparator: \.isNewline).reversed().first { $0.hasSuffix("total") }
    if let line = total, let token = line.split(whereSeparator: \.isWhitespace).first, let kb = UInt64(token) {
        return .success(kb * 1024)
    }
    let err = String(data: stderr.fileHandleForReading.readDataToEndOfFile(), encoding: .utf8) ?? ""
    return .failure(AppError(err.split(whereSeparator: \.isNewline).first.map(String.init) ?? "du failed"))
}
