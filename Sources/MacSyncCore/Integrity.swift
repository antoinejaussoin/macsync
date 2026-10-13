import Foundation

private let listCap = 200

struct Differences: Equatable {
    var silent: [String] = []
    var changed: [String] = []
    var missing: [String] = []
    var extra: [String] = []
    var metadataOnly: Int = 0
}

func classifyItemized(_ lines: [String]) -> Differences {
    var differences = Differences()
    for line in lines {
        guard let split = line.split(separator: "|", maxSplits: 1, omittingEmptySubsequences: false).map(String.init) as [String]?,
              split.count == 2
        else { continue }
        let item = split[0].trimmingCharacters(in: .whitespaces)
        let name = split[1]
        if item.hasPrefix("*deleting") {
            if !name.hasSuffix("/") { differences.extra.append(name) }
            continue
        }
        let flags = Array(item)
        if flags.count < 5 || flags[1] == "d" { continue }
        if flags.count >= 5, flags[2] == "+", flags[3] == "+", flags[4] == "+" {
            differences.missing.append(name)
        } else if flags[0] == ">" || flags[0] == "c" {
            if flags[2] == "c", flags[3] == ".", flags[4] == "." {
                differences.silent.append(name)
            } else {
                differences.changed.append(name)
            }
        } else {
            differences.metadataOnly += 1
        }
    }
    return differences
}

private func capped(_ values: [String]) -> (list: [String], count: Int) {
    (Array(values.prefix(listCap)), values.count)
}

func integrityRecord(from differences: Differences, snapshot: String) -> IntegrityRecord {
    let outcome: Outcome
    let message: String
    if !differences.silent.isEmpty {
        outcome = .failed
        message = "\(differences.silent.count) file(s) differ with the same size and date: possible corruption or tampering"
    } else if differences.changed.isEmpty && differences.missing.isEmpty && differences.extra.isEmpty {
        outcome = .success
        message = "Latest snapshot matches the source byte for byte"
    } else {
        outcome = .warning
        message = "Source changed since the snapshot: \(differences.changed.count) edited, \(differences.missing.count) new, \(differences.extra.count) deleted"
    }
    let (silent, silentCount) = capped(differences.silent)
    let (changed, changedCount) = capped(differences.changed)
    let (missing, missingCount) = capped(differences.missing)
    let (extra, extraCount) = capped(differences.extra)
    return IntegrityRecord(
        started: nil,
        finished: nil,
        outcome: outcome,
        message: message,
        snapshot: snapshot,
        silent: silent,
        silentCount: silentCount,
        changed: changed,
        changedCount: changedCount,
        missing: missing,
        missingCount: missingCount,
        extra: extra,
        extraCount: extraCount
    )
}

func runIntegrity(pair: Pair, rsync: RsyncInfo, log: LogFile, cancel: CancelHandle) -> Result<IntegrityRecord, AppError> {
    let started = Date()
    let source: String
    let destPath: String
    switch checkPairPaths(source: pair.source, destination: pair.destination) {
    case .success(let paths):
        source = paths.0
        destPath = paths.1
    case .failure(let error):
        return .failure(AppError(error.description))
    }
    let dest = URL(fileURLWithPath: destPath, isDirectory: true)
    guard let snapshot = latestSnapshotName(dest: dest) else {
        return .failure(AppError("No snapshot to check yet"))
    }
    let args = integrityArguments(
        source: source,
        snapshot: dest.appendingPathComponent(snapshot).path,
        excludes: pair.excludes,
        readOnly: pair.readOnly
    )
    log.line("Integrity check of \(snapshot) against \(source)")
    log.line("\(rsync.path) \(displayArguments(args))")
    let output: RunOutput
    do {
        output = try runRsync(path: rsync.path, arguments: args, log: log, cancel: cancel, keepStdout: true, onProgress: { _ in })
    } catch {
        return .failure(AppError(error.localizedDescription))
    }
    if cancel.isCancelled { return .failure(AppError("Cancelled")) }
    var record = integrityRecord(from: classifyItemized(output.stdoutLines), snapshot: snapshot)
    switch output.exitCode {
    case 0, 24:
        break
    default:
        let detail = output.stderrTail.last ?? ""
        record.outcome = .failed
        record.message = "rsync exit \(output.exitCode): \(rsyncExitMeaning(output.exitCode)) \(detail). \(record.message)"
    }
    record.started = started
    record.finished = Date()
    log.line("Integrity: \(record.message)")
    return .success(record)
}
