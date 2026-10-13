import Foundation

struct SyncState: Equatable {
    var pairs: [String: PairState]

    static let empty = SyncState(pairs: [:])

    func load() -> SyncState { self }

    static func load(from url: URL = Paths.stateFile) -> SyncState {
        guard let text = try? String(contentsOf: url, encoding: .utf8) else { return .empty }
        guard let table = try? TOMLTable.parse(text), let state = try? SyncState(parsing: table) else {
            return .empty
        }
        return state
    }

    func save(to url: URL = Paths.stateFile) throws {
        try writeAtomic(toml(), to: url)
    }
}

struct PairState: Equatable {
    var lastBackup: RunRecord?
    var lastSuccess: Date?
    var lastIntegrity: IntegrityRecord?
    var stats: Stats?

    static let empty = PairState(lastBackup: nil, lastSuccess: nil, lastIntegrity: nil, stats: nil)
}

enum Outcome: String, Equatable {
    case success, warning, failed
}

struct RunRecord: Equatable {
    var started: Date
    var finished: Date
    var outcome: Outcome
    var message: String
    var snapshot: String?
    var pruned: Int
}

struct IntegrityRecord: Equatable {
    var started: Date?
    var finished: Date?
    var outcome: Outcome?
    var message: String
    var snapshot: String
    var silent: [String]
    var silentCount: Int
    var changed: [String]
    var changedCount: Int
    var missing: [String]
    var missingCount: Int
    var extra: [String]
    var extraCount: Int

    static let empty = IntegrityRecord(
        started: nil, finished: nil, outcome: nil, message: "", snapshot: "",
        silent: [], silentCount: 0, changed: [], changedCount: 0,
        missing: [], missingCount: 0, extra: [], extraCount: 0
    )
}

struct Stats: Equatable {
    var measured: Date?
    var sourceBytes: UInt64?
    var destinationBytes: UInt64?
    var freeBytes: UInt64?
    var filesystem: String?
    var snapshotCount: Int
    var oldest: String?
    var newest: String?
    var error: String?

    static let empty = Stats(
        measured: nil, sourceBytes: nil, destinationBytes: nil, freeBytes: nil,
        filesystem: nil, snapshotCount: 0, oldest: nil, newest: nil, error: nil
    )
}

extension SyncState {
    init(parsing table: TOMLTable) throws {
        try table.rejectUnknown(in: ["pairs"])
        pairs = [:]
        guard let pairsTable = try table.optionalTable("pairs") else { return }
        for (id, value) in pairsTable.values {
            guard case .table(let pairTable) = value else {
                throw TOMLError("state for \(id) must be a table")
            }
            pairs[id] = try PairState(parsing: pairTable)
        }
    }

    func toml() -> String {
        let ids = pairs.keys.sorted()
        if ids.isEmpty { return "[pairs]\n" }
        var lines: [String] = []
        for id in ids {
            lines.append(contentsOf: pairs[id]!.tomlLines(id: id))
        }
        return lines.joined(separator: "\n") + "\n"
    }
}

extension PairState {
    init(parsing table: TOMLTable) throws {
        try table.rejectUnknown(in: ["last_backup", "last_success", "last_integrity", "stats"])
        if let backup = try table.optionalTable("last_backup") {
            lastBackup = try RunRecord(parsing: backup)
        }
        lastSuccess = try table.optionalDate("last_success")
        if let integrity = try table.optionalTable("last_integrity") {
            lastIntegrity = try IntegrityRecord(parsing: integrity)
        }
        if let statsTable = try table.optionalTable("stats") {
            stats = try Stats(parsing: statsTable)
        }
    }

    func tomlLines(id: String) -> [String] {
        var lines: [String] = ["[pairs.\(tomlKey(id))]"]
        if let lastSuccess {
            lines.append("last_success = \(tomlString(formatRFC3339(lastSuccess)))")
        }
        if lastBackup == nil, lastIntegrity == nil, stats == nil {
            lines.append("")
            return lines
        }
        lines.append("")
        if let lastBackup {
            lines.append("[pairs.\(tomlKey(id)).last_backup]")
            lines.append(contentsOf: lastBackup.tomlLines())
            lines.append("")
        }
        if let lastIntegrity {
            lines.append("[pairs.\(tomlKey(id)).last_integrity]")
            lines.append(contentsOf: lastIntegrity.tomlLines())
            lines.append("")
        }
        if let stats {
            lines.append("[pairs.\(tomlKey(id)).stats]")
            lines.append(contentsOf: stats.tomlLines())
            lines.append("")
        }
        return lines
    }
}

extension RunRecord {
    init(parsing table: TOMLTable) throws {
        try table.rejectUnknown(in: ["started", "finished", "outcome", "message", "snapshot", "pruned"])
        guard let started = try table.optionalDate("started"), let finished = try table.optionalDate("finished") else {
            throw TOMLError("last_backup is missing started or finished")
        }
        guard let outcomeName = try table.optionalString("outcome"), let outcome = Outcome(rawValue: outcomeName) else {
            throw TOMLError("last_backup has an invalid outcome")
        }
        self.started = started
        self.finished = finished
        self.outcome = outcome
        message = try table.optionalString("message") ?? ""
        snapshot = try table.optionalString("snapshot")
        pruned = try table.optionalInt("pruned") ?? 0
    }

    func tomlLines() -> [String] {
        var lines = [
            "started = \(tomlString(formatRFC3339(started)))",
            "finished = \(tomlString(formatRFC3339(finished)))",
            "outcome = \(tomlString(outcome.rawValue))",
            "message = \(tomlString(message))",
        ]
        if let snapshot {
            lines.append("snapshot = \(tomlString(snapshot))")
        }
        lines.append("pruned = \(pruned)")
        return lines
    }
}

extension IntegrityRecord {
    init(parsing table: TOMLTable) throws {
        try table.rejectUnknown(in: [
            "started", "finished", "outcome", "message", "snapshot",
            "silent", "silent_count", "changed", "changed_count",
            "missing", "missing_count", "extra", "extra_count",
        ])
        started = try table.optionalDate("started")
        finished = try table.optionalDate("finished")
        if let name = try table.optionalString("outcome") {
            guard let outcome = Outcome(rawValue: name) else {
                throw TOMLError("invalid integrity outcome \"\(name)\"")
            }
            self.outcome = outcome
        }
        message = try table.optionalString("message") ?? ""
        snapshot = try table.optionalString("snapshot") ?? ""
        silent = table.values["silent"] == nil ? [] : try table.stringArray("silent")
        silentCount = try table.optionalInt("silent_count") ?? silent.count
        changed = table.values["changed"] == nil ? [] : try table.stringArray("changed")
        changedCount = try table.optionalInt("changed_count") ?? changed.count
        missing = table.values["missing"] == nil ? [] : try table.stringArray("missing")
        missingCount = try table.optionalInt("missing_count") ?? missing.count
        extra = table.values["extra"] == nil ? [] : try table.stringArray("extra")
        extraCount = try table.optionalInt("extra_count") ?? extra.count
    }

    func tomlLines() -> [String] {
        var lines: [String] = []
        if let started { lines.append("started = \(tomlString(formatRFC3339(started)))") }
        if let finished { lines.append("finished = \(tomlString(formatRFC3339(finished)))") }
        if let outcome { lines.append("outcome = \(tomlString(outcome.rawValue))") }
        lines.append("message = \(tomlString(message))")
        lines.append("snapshot = \(tomlString(snapshot))")
        lines.append("silent = \(tomlStringArray(silent))")
        lines.append("silent_count = \(silentCount)")
        lines.append("changed = \(tomlStringArray(changed))")
        lines.append("changed_count = \(changedCount)")
        lines.append("missing = \(tomlStringArray(missing))")
        lines.append("missing_count = \(missingCount)")
        lines.append("extra = \(tomlStringArray(extra))")
        lines.append("extra_count = \(extraCount)")
        return lines
    }
}

extension Stats {
    init(parsing table: TOMLTable) throws {
        try table.rejectUnknown(in: [
            "measured", "source_bytes", "destination_bytes", "free_bytes", "filesystem",
            "snapshot_count", "oldest", "newest", "error",
        ])
        measured = try table.optionalDate("measured")
        sourceBytes = try table.optionalUInt("source_bytes")
        destinationBytes = try table.optionalUInt("destination_bytes")
        freeBytes = try table.optionalUInt("free_bytes")
        filesystem = try table.optionalString("filesystem")
        snapshotCount = try table.optionalInt("snapshot_count") ?? 0
        oldest = try table.optionalString("oldest")
        newest = try table.optionalString("newest")
        error = try table.optionalString("error")
    }

    func tomlLines() -> [String] {
        var lines: [String] = []
        if let measured { lines.append("measured = \(tomlString(formatRFC3339(measured)))") }
        if let sourceBytes { lines.append("source_bytes = \(sourceBytes)") }
        if let destinationBytes { lines.append("destination_bytes = \(destinationBytes)") }
        if let freeBytes { lines.append("free_bytes = \(freeBytes)") }
        if let filesystem { lines.append("filesystem = \(tomlString(filesystem))") }
        lines.append("snapshot_count = \(snapshotCount)")
        if let oldest { lines.append("oldest = \(tomlString(oldest))") }
        if let newest { lines.append("newest = \(tomlString(newest))") }
        if let error { lines.append("error = \(tomlString(error))") }
        return lines
    }
}
