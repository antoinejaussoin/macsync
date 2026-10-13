import Foundation

struct Config: Equatable {
    var rsyncPath: String?
    var pairs: [Pair]

    static let empty = Config(rsyncPath: nil, pairs: [])

    func pair(_ id: String) -> Pair? {
        pairs.first { $0.id == id }
    }
}

struct Pair: Equatable, Identifiable {
    var id: String
    var name: String
    var source: String
    var destination: String
    var enabled: Bool
    var backupSchedule: String
    var integritySchedule: String
    var excludes: [String]
    var readOnly: Bool
    var retention: Retention

    static func makeNew() -> Pair {
        Pair(
            id: String(UUID().uuidString.replacingOccurrences(of: "-", with: "").lowercased().prefix(12)),
            name: "New backup",
            source: "",
            destination: "",
            enabled: true,
            backupSchedule: "0 2 * * *",
            integritySchedule: "",
            excludes: [".DS_Store"],
            readOnly: true,
            retention: .default
        )
    }

    /// Problems that make the pair unsavable. Folder existence is checked at run time.
    func validate() -> [String] {
        var errors: [String] = []
        if name.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty {
            errors.append("Name is empty")
        }
        if case .failure(let error) = checkPairPaths(source: source, destination: destination) {
            switch error {
            case .notSet, .unsafe:
                errors.append(error.description)
            case .sourceMissing, .destinationMissing:
                break
            }
        }
        if let error = validateSchedule(backupSchedule) {
            errors.append("Backup schedule: \(error)")
        }
        if let error = validateSchedule(integritySchedule) {
            errors.append("Integrity schedule: \(error)")
        }
        return errors
    }

    var displayName: String {
        let trimmed = name.trimmingCharacters(in: .whitespacesAndNewlines)
        return trimmed.isEmpty ? "Untitled" : trimmed
    }
}

func loadConfig(from url: URL = Paths.configFile) -> (Config, String?) {
    let text: String
    do {
        text = try String(contentsOf: url, encoding: .utf8)
    } catch {
        let nsError = error as NSError
        if nsError.domain == NSCocoaErrorDomain, nsError.code == NSFileReadNoSuchFileError {
            return (.empty, nil)
        }
        if (error as? CocoaError)?.code == .fileReadNoSuchFile {
            return (.empty, nil)
        }
        return (.empty, "Could not read \(url.path): \(error)")
    }
    do {
        let table = try TOMLTable.parse(text)
        return (try Config(parsing: table), nil)
    } catch {
        let stamp = brokenStamp(Date())
        let aside = url.deletingPathExtension().appendingPathExtension("toml.broken-\(stamp)")
        try? FileManager.default.moveItem(at: url, to: aside)
        return (.empty, "Config was invalid and was moved to \(aside.path): \(error)")
    }
}

func saveConfig(_ config: Config, to url: URL = Paths.configFile) throws {
    try writeAtomic(config.toml(), to: url)
}

private func brokenStamp(_ date: Date) -> String {
    let parts = Calendar.current.dateComponents([.year, .month, .day, .hour, .minute, .second], from: date)
    return String(
        format: "%04d%02d%02d%02d%02d%02d",
        parts.year ?? 0, parts.month ?? 0, parts.day ?? 0,
        parts.hour ?? 0, parts.minute ?? 0, parts.second ?? 0
    )
}

func writeAtomic(_ text: String, to url: URL) throws {
    let directory = url.deletingLastPathComponent()
    do {
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
    } catch {
        throw AppError("Could not create \(directory.path): \(error)")
    }
    let tmp = url.deletingPathExtension().appendingPathExtension("tmp")
    do {
        try text.write(to: tmp, atomically: false, encoding: .utf8)
    } catch {
        throw AppError("Could not write \(tmp.path): \(error)")
    }
    do {
        if FileManager.default.fileExists(atPath: url.path) {
            _ = try FileManager.default.replaceItemAt(url, withItemAt: tmp)
        } else {
            try FileManager.default.moveItem(at: tmp, to: url)
        }
    } catch {
        throw AppError("Could not replace \(url.path): \(error)")
    }
}

struct AppError: LocalizedError, Equatable {
    var message: String
    init(_ message: String) { self.message = message }
    var errorDescription: String? { message }
    var description: String { message }
}

extension Config {
    init(parsing table: TOMLTable) throws {
        try table.rejectUnknown(in: ["rsync_path", "pair"])
        rsyncPath = try table.optionalString("rsync_path")
        if rsyncPath?.isEmpty == true { rsyncPath = nil }
        if table.values["pair"] == nil {
            pairs = []
            return
        }
        let array = try table.array("pair")
        pairs = try array.map { value in
            guard case .table(let pairTable) = value else {
                throw TOMLError("pair entries must be tables")
            }
            return try Pair(parsing: pairTable)
        }
    }

    func toml() -> String {
        var lines: [String] = []
        if let rsyncPath, !rsyncPath.isEmpty {
            lines.append("rsync_path = \(tomlString(rsyncPath))")
            lines.append("")
        }
        if pairs.isEmpty {
            lines.append("pair = []")
            lines.append("")
            return lines.joined(separator: "\n")
        }
        for pair in pairs {
            if !lines.isEmpty, lines.last != "" { lines.append("") }
            lines.append(contentsOf: pair.tomlLines())
            lines.append("")
        }
        return lines.joined(separator: "\n")
    }
}

extension Pair {
    init(parsing table: TOMLTable) throws {
        try table.rejectUnknown(in: [
            "id", "name", "source", "destination", "enabled",
            "backup_schedule", "integrity_schedule", "excludes", "read_only", "retention",
        ])
        let fresh = Pair.makeNew()
        id = try table.optionalString("id") ?? fresh.id
        name = try table.optionalString("name") ?? fresh.name
        source = try table.optionalString("source") ?? ""
        destination = try table.optionalString("destination") ?? ""
        enabled = try table.optionalBool("enabled") ?? true
        backupSchedule = try table.optionalString("backup_schedule") ?? fresh.backupSchedule
        integritySchedule = try table.optionalString("integrity_schedule") ?? ""
        if table.values["excludes"] == nil {
            excludes = fresh.excludes
        } else {
            excludes = try table.stringArray("excludes")
        }
        readOnly = try table.optionalBool("read_only") ?? true
        if let retentionTable = try table.optionalTable("retention") {
            retention = try Retention(parsing: retentionTable)
        } else {
            retention = .default
        }
    }

    func tomlLines() -> [String] {
        let lines = [
            "[[pair]]",
            "id = \(tomlString(id))",
            "name = \(tomlString(name))",
            "source = \(tomlString(source))",
            "destination = \(tomlString(destination))",
            "enabled = \(enabled ? "true" : "false")",
            "backup_schedule = \(tomlString(backupSchedule))",
            "integrity_schedule = \(tomlString(integritySchedule))",
            "excludes = \(tomlStringArray(excludes))",
            "read_only = \(readOnly ? "true" : "false")",
            "",
            "[pair.retention]",
            "keep_last = \(retention.keepLast)",
            "keep_daily = \(retention.keepDaily)",
            "keep_weekly = \(retention.keepWeekly)",
            "keep_monthly = \(retention.keepMonthly)",
            "keep_yearly = \(retention.keepYearly.toml())",
        ]
        return lines
    }
}

extension Retention {
    init(parsing table: TOMLTable) throws {
        try table.rejectUnknown(in: ["keep_last", "keep_daily", "keep_weekly", "keep_monthly", "keep_yearly"])
        let base = Retention.default
        keepLast = try table.optionalInt("keep_last") ?? base.keepLast
        keepDaily = try table.optionalInt("keep_daily") ?? base.keepDaily
        keepWeekly = try table.optionalInt("keep_weekly") ?? base.keepWeekly
        keepMonthly = try table.optionalInt("keep_monthly") ?? base.keepMonthly
        if table.values["keep_yearly"] == nil {
            keepYearly = base.keepYearly
        } else {
            keepYearly = try KeepYearly(parsing: table.values["keep_yearly"]!)
        }
    }
}

extension KeepYearly {
    init(parsing value: TOMLValue) throws {
        switch value {
        case .int(let number):
            self = .count(Int(number))
        case .string(let word) where word.lowercased() == "forever":
            self = .forever
        case .string(let word):
            throw TOMLError("keep_yearly must be \"forever\" or a number, got \"\(word)\"")
        default:
            throw TOMLError("keep_yearly must be \"forever\" or a number")
        }
    }

    func toml() -> String {
        switch self {
        case .forever: return "\"forever\""
        case .count(let count): return "\(count)"
        }
    }
}
