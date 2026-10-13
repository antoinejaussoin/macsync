import Foundation

enum Paths {
    /// `HOME`, or `/` when it is unset. Matches the previous app.
    static var home: String {
        let value = ProcessInfo.processInfo.environment["HOME"] ?? ""
        return value.isEmpty ? "/" : value
    }

    /// `MACSYNC_HOME` relocates config, state and logs (used to run a second instance).
    static var appSupport: URL {
        if let dir = ProcessInfo.processInfo.environment["MACSYNC_HOME"], !dir.isEmpty {
            return URL(fileURLWithPath: dir, isDirectory: true)
        }
        return URL(fileURLWithPath: home, isDirectory: true)
            .appendingPathComponent("Library/Application Support/MacSync", isDirectory: true)
    }

    static var logs: URL {
        if let dir = ProcessInfo.processInfo.environment["MACSYNC_HOME"], !dir.isEmpty {
            return URL(fileURLWithPath: dir, isDirectory: true).appendingPathComponent("Logs", isDirectory: true)
        }
        return URL(fileURLWithPath: home, isDirectory: true)
            .appendingPathComponent("Library/Logs/MacSync", isDirectory: true)
    }

    static var configFile: URL { appSupport.appendingPathComponent("config.toml") }
    static var stateFile: URL { appSupport.appendingPathComponent("state.toml") }
    static var lockFile: URL { appSupport.appendingPathComponent("macsync.lock") }
    static var socketFile: URL { appSupport.appendingPathComponent("macsync.sock") }

    static func logFile(pairID: String) -> URL {
        logs.appendingPathComponent("\(pairID).log")
    }
}

enum PathError: Error, Equatable, CustomStringConvertible {
    case notSet(String)
    case sourceMissing(String)
    case destinationMissing(String)
    case unsafe(String)

    var description: String {
        switch self {
        case .notSet(let what):
            return "\(what) folder is not set"
        case .sourceMissing(let path):
            return "Source folder not found: \(path)"
        case .destinationMissing(let path):
            return "Destination not available: \(path) (is the drive connected?)"
        case .unsafe(let message):
            return message
        }
    }
}

/// Resolves both folders and refuses combinations where `--delete` or snapshot
/// pruning could touch the source. The destination must already exist so an
/// unmounted volume never turns into a new folder under `/Volumes`.
func checkPairPaths(source: String, destination: String) -> Result<(String, String), PathError> {
    if source.isEmpty { return .failure(.notSet("Source")) }
    if destination.isEmpty { return .failure(.notSet("Destination")) }
    if !source.hasPrefix("/") || !destination.hasPrefix("/") {
        return .failure(.unsafe("Source and destination must be absolute paths"))
    }
    if let error = lexicalSafety(source: source, destination: destination) {
        return .failure(error)
    }
    var isDir: ObjCBool = false
    if !FileManager.default.fileExists(atPath: source, isDirectory: &isDir) || !isDir.boolValue {
        return .failure(.sourceMissing(source))
    }
    isDir = false
    if !FileManager.default.fileExists(atPath: destination, isDirectory: &isDir) || !isDir.boolValue {
        return .failure(.destinationMissing(destination))
    }
    guard let src = canonicalize(source) else { return .failure(.sourceMissing(source)) }
    guard let dst = canonicalize(destination) else { return .failure(.destinationMissing(destination)) }
    if let error = lexicalSafety(source: src, destination: dst) {
        return .failure(error)
    }
    return .success((src, dst))
}

func canonicalize(_ path: String) -> String? {
    path.withCString { cString in
        guard let resolved = realpath(cString, nil) else { return nil }
        defer { free(resolved) }
        return String(cString: resolved)
    }
}

private func lexicalSafety(source: String, destination: String) -> PathError? {
    if samePath(destination, "/") {
        return .unsafe("Destination cannot be the root folder /")
    }
    if samePath(destination, Paths.home) {
        return .unsafe("Destination cannot be your home folder itself")
    }
    if samePath(source, destination) {
        return .unsafe("Source and destination are the same folder")
    }
    if isInside(destination, ancestor: source) {
        return .unsafe("Destination is inside the source folder")
    }
    if isInside(source, ancestor: destination) {
        return .unsafe("Source is inside the destination folder")
    }
    return nil
}

func pathComponents(_ path: String) -> [String] {
    path.split(separator: "/", omittingEmptySubsequences: true).map(String.init)
}

func samePath(_ a: String, _ b: String) -> Bool {
    pathComponents(a) == pathComponents(b)
}

/// Strict containment: `/Docs` does not contain `/Docs-backup`.
func isInside(_ path: String, ancestor: String) -> Bool {
    let child = pathComponents(path)
    let parent = pathComponents(ancestor)
    guard child.count > parent.count else { return false }
    return Array(child.prefix(parent.count)) == parent
}
