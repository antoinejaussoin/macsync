import Foundation
import UserNotifications

enum JobKind: String, Equatable, Hashable {
    case backup
    case integrity

    var verb: String {
        switch self {
        case .backup: return "Backing up"
        case .integrity: return "Checking"
        }
    }

    func schedule(of pair: Pair) -> String {
        switch self {
        case .backup: return pair.backupSchedule
        case .integrity: return pair.integritySchedule
        }
    }
}

struct Job: Equatable {
    var pairID: String
    var kind: JobKind
}

struct Running: Equatable {
    var job: Job
    var pairName: String
    var started: Date
    var progress: TransferProgress?
}

struct AnchorKey: Hashable {
    var pairID: String
    var kind: JobKind
}

struct SchedulerData {
    var config: Config
    var state: SyncState
    var queue: [Job] = []
    var running: Running?
    var rsync: RsyncStatus
    var statsBusy: Set<String> = []
    var waitingForDestination: Set<String> = []
    var notice: String?
    var anchors: [AnchorKey: Date] = [:]

    func lastAttempt(_ pairID: String, _ kind: JobKind) -> Date? {
        guard let pair = state.pairs[pairID] else { return nil }
        switch kind {
        case .backup: return pair.lastBackup?.started
        case .integrity: return pair.lastIntegrity?.started
        }
    }

    mutating func anchor(_ pairID: String, _ kind: JobKind, now: Date) -> Date {
        if let last = lastAttempt(pairID, kind) { return last }
        let key = AnchorKey(pairID: pairID, kind: kind)
        if let existing = anchors[key] { return existing }
        anchors[key] = now
        return now
    }

    func nextRun(pair: Pair, kind: JobKind, now: Date = Date()) -> Date? {
        let anchor = lastAttempt(pair.id, kind) ?? anchors[AnchorKey(pairID: pair.id, kind: kind)] ?? now
        return nextAfter(kind.schedule(of: pair), after: anchor)
    }

    func isPending(_ pairID: String, _ kind: JobKind) -> Bool {
        let job = Job(pairID: pairID, kind: kind)
        return queue.contains(job) || running?.job == job
    }

    func isBusy(_ pairID: String) -> Bool {
        isPending(pairID, .backup) || isPending(pairID, .integrity)
    }

    mutating func push(pairID: String, kind: JobKind) -> Bool {
        if isPending(pairID, kind) { return false }
        queue.append(Job(pairID: pairID, kind: kind))
        return true
    }

    mutating func tick(now: Date) -> Bool {
        var changed = false
        let pairs = config.pairs.filter(\.enabled)
        for pair in pairs {
            for kind in [JobKind.backup, JobKind.integrity] {
                let expression = kind.schedule(of: pair)
                if expression.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty { continue }
                let anchor = anchor(pair.id, kind, now: now)
                if !scheduleIsDue(expression, anchor: anchor, now: now) || isPending(pair.id, kind) { continue }
                var isDir: ObjCBool = false
                let available = FileManager.default.fileExists(atPath: pair.destination, isDirectory: &isDir) && isDir.boolValue
                if !available {
                    if waitingForDestination.insert(pair.id).inserted { changed = true }
                    continue
                }
                if waitingForDestination.remove(pair.id) != nil { changed = true }
                if push(pairID: pair.id, kind: kind) { changed = true }
            }
        }
        return changed
    }
}

struct EngineSnapshot: Equatable {
    var config: Config
    var state: SyncState
    var running: Running?
    var queue: [Job]
    var rsync: RsyncStatus
    var waiting: Set<String>
    var statsBusy: Set<String>
    var notice: String?
    var nextRuns: [AnchorKey: Date]
}

final class Engine: @unchecked Sendable {
    private var shared: SchedulerData
    private let cond = NSCondition()
    private var currentCancel = CancelHandle()
    private var onChange: (() -> Void)?
    /// Size measurements waiting until no rsync is running, so `du` does not compete with a copy.
    private var pendingMeasure: Set<String> = []
    private let tickInterval: TimeInterval

    init(config: Config, state: SyncState, notice: String?, tickInterval: TimeInterval = 15) {
        shared = SchedulerData(config: config, state: state, rsync: locateRsync(overridePath: config.rsyncPath), notice: notice)
        self.tickInterval = tickInterval
    }

    func setOnChange(_ handler: @escaping () -> Void) {
        onChange = handler
    }

    func start() {
        UNUserNotificationCenter.current().requestAuthorization(options: [.alert, .sound]) { _, _ in }
        let worker = Thread { [weak self] in self?.workerLoop() }
        worker.name = "macsync-worker"
        worker.start()
        let ticker = Thread { [weak self] in self?.tickerLoop() }
        ticker.name = "macsync-scheduler"
        ticker.start()
    }

    func snapshot() -> EngineSnapshot {
        withLock {
            var nextRuns: [AnchorKey: Date] = [:]
            let now = Date()
            for pair in shared.config.pairs {
                for kind in [JobKind.backup, JobKind.integrity] {
                    if let next = shared.nextRun(pair: pair, kind: kind, now: now) {
                        nextRuns[AnchorKey(pairID: pair.id, kind: kind)] = next
                    }
                }
            }
            return EngineSnapshot(
                config: shared.config,
                state: shared.state,
                running: shared.running,
                queue: shared.queue,
                rsync: shared.rsync,
                waiting: shared.waitingForDestination,
                statsBusy: shared.statsBusy,
                notice: shared.notice,
                nextRuns: nextRuns
            )
        }
    }

    func enqueue(pairID: String, kind: JobKind) {
        let added = withLock { shared.push(pairID: pairID, kind: kind) }
        if added {
            cond.lock()
            cond.signal()
            cond.unlock()
            changed()
        }
    }

    func enqueueAllBackups() {
        let added = withLock { () -> Bool in
            var any = false
            for pair in shared.config.pairs where pair.enabled {
                if shared.push(pairID: pair.id, kind: .backup) { any = true }
            }
            return any
        }
        if added {
            cond.lock()
            cond.signal()
            cond.unlock()
            changed()
        }
    }

    func cancel(pairID: String) {
        let handle: CancelHandle? = withLock {
            shared.queue.removeAll { $0.pairID == pairID }
            if shared.running?.job.pairID == pairID { return currentCancel }
            return nil
        }
        handle?.cancel()
        changed()
    }

    func replaceConfig(_ config: Config) throws {
        try saveConfig(config)
        withLock {
            let ids = Set(config.pairs.map(\.id))
            shared.queue.removeAll { !ids.contains($0.pairID) }
            shared.waitingForDestination = shared.waitingForDestination.filter { ids.contains($0) }
            shared.rsync = locateRsync(overridePath: config.rsyncPath)
            shared.config = config
        }
        changed()
    }

    func refreshRsync() {
        withLock { shared.rsync = locateRsync(overridePath: shared.config.rsyncPath) }
        changed()
    }

    func dismissNotice() {
        withLock { shared.notice = nil }
        changed()
    }

    func refreshStats(pairID: String) {
        let pair: Pair? = withLock {
            guard let pair = shared.config.pair(pairID) else { return nil }
            if shared.running != nil || !shared.queue.isEmpty {
                pendingMeasure.insert(pairID)
                return nil
            }
            if !shared.statsBusy.insert(pairID).inserted { return nil }
            return pair
        }
        guard let pair else { return }
        changed()
        DispatchQueue.global(qos: .utility).async { [weak self] in
            guard let self else { return }
            let measured = measureStats(pair: pair)
            self.withLock {
                self.shared.statsBusy.remove(pair.id)
                var record = self.shared.state.pairs[pair.id] ?? .empty
                record.stats = measured
                self.shared.state.pairs[pair.id] = record
                try? self.shared.state.save()
            }
            self.changed()
        }
    }

    func removePairState(_ id: String) {
        withLock {
            shared.state.pairs.removeValue(forKey: id)
            try? shared.state.save()
        }
    }

    func shutdown() {
        let handle = withLock { currentCancel }
        handle.cancel()
        withLock { try? shared.state.save() }
    }

    private func tickerLoop() {
        while true {
            let didChange = withLock { shared.tick(now: Date()) }
            if didChange {
                cond.lock()
                cond.signal()
                cond.unlock()
                changed()
            }
            Thread.sleep(forTimeInterval: tickInterval)
        }
    }

    private func workerLoop() {
        while true {
            let work = nextWork()
            changed()
            let cancel = CancelHandle()
            withLock { currentCancel = cancel }
            var measurePair: String?
            switch work.job.kind {
            case .backup:
                if runBackupJob(pair: work.pair, rsync: work.rsync, cancel: cancel, started: work.started) {
                    measurePair = work.pair.id
                }
            case .integrity:
                runIntegrityJob(pair: work.pair, rsync: work.rsync, cancel: cancel, started: work.started)
            }
            withLock {
                shared.running = nil
                if let measurePair { pendingMeasure.insert(measurePair) }
            }
            changed()
            flushPendingMeasure()
        }
    }

    /// Disk usage walks the trees, so it waits until the copy queue is empty.
    private func flushPendingMeasure() {
        let ids: [String] = withLock {
            guard shared.running == nil, shared.queue.isEmpty else { return [] }
            let ids = pendingMeasure
            pendingMeasure.removeAll()
            return Array(ids)
        }
        for id in ids {
            refreshStats(pairID: id)
        }
    }

    private struct Work {
        var job: Job
        var pair: Pair
        var rsync: RsyncStatus
        var started: Date
    }

    private func nextWork() -> Work {
        cond.lock()
        defer { cond.unlock() }
        while true {
            if !shared.queue.isEmpty {
                let job = shared.queue.removeFirst()
                if let pair = shared.config.pair(job.pairID) {
                    shared.rsync = locateRsync(overridePath: shared.config.rsyncPath)
                    let started = Date()
                    shared.running = Running(job: job, pairName: pair.displayName, started: started, progress: nil)
                    return Work(job: job, pair: pair, rsync: shared.rsync, started: started)
                }
                continue
            }
            cond.wait()
        }
    }

    @discardableResult
    private func runBackupJob(pair: Pair, rsync: RsyncStatus, cancel: CancelHandle, started: Date) -> Bool {
        var lastUpdate = Date().addingTimeInterval(-5)
        let result: Result<BackupResult, BackupError>
        switch rsync {
        case .missing(let message):
            result = .failure(.failed(message))
        case .found(let info):
            do {
                let log = try LogFile(pairID: pair.id)
                result = runBackup(pair: pair, rsync: info, log: log, cancel: cancel) { progress in
                    self.withLock {
                        if var running = self.shared.running {
                            running.progress = progress
                            self.shared.running = running
                        }
                    }
                    if Date().timeIntervalSince(lastUpdate) >= 0.5 {
                        lastUpdate = Date()
                        self.changed()
                    }
                }
            } catch {
                result = .failure(.failed(error.localizedDescription))
            }
        }
        let finished = Date()
        let record: RunRecord
        switch result {
        case .success(let backup):
            record = RunRecord(
                started: started,
                finished: finished,
                outcome: backup.warning == nil ? .success : .warning,
                message: backup.warning.map { "\(backup.summary); \($0)" } ?? backup.summary,
                snapshot: backup.snapshot,
                pruned: backup.pruned.count
            )
        case .failure(let error):
            record = RunRecord(
                started: started,
                finished: finished,
                outcome: .failed,
                message: error.description,
                snapshot: nil,
                pruned: 0
            )
        }
        let notifyFailure: Bool
        if case .failure(let error) = result {
            switch error {
            case .failed, .destinationMissing: notifyFailure = true
            case .cancelled: notifyFailure = false
            }
        } else {
            notifyFailure = false
        }
        withLock {
            var recordState = shared.state.pairs[pair.id] ?? .empty
            if case .success = result { recordState.lastSuccess = finished }
            recordState.lastBackup = record
            shared.state.pairs[pair.id] = recordState
            try? shared.state.save()
        }
        if notifyFailure {
            postNotification(summary: "Backup failed: \(pair.displayName)", body: record.message)
        }
        if case .success = result { return true }
        return false
    }

    private func runIntegrityJob(pair: Pair, rsync: RsyncStatus, cancel: CancelHandle, started: Date) {
        let result: Result<IntegrityRecord, AppError>
        switch rsync {
        case .missing(let message):
            result = .failure(AppError(message))
        case .found(let info):
            do {
                let log = try LogFile(pairID: pair.id)
                result = runIntegrity(pair: pair, rsync: info, log: log, cancel: cancel)
            } catch {
                result = .failure(AppError(error.localizedDescription))
            }
        }
        var record = result.unwrapOr { error in
            var failed = IntegrityRecord.empty
            failed.outcome = .failed
            failed.message = error.message
            return failed
        }
        record.started = started
        record.finished = Date()
        if record.outcome == .failed, !cancel.isCancelled {
            postNotification(summary: "Integrity check: \(pair.displayName)", body: record.message)
        }
        withLock {
            var recordState = shared.state.pairs[pair.id] ?? .empty
            recordState.lastIntegrity = record
            shared.state.pairs[pair.id] = recordState
            try? shared.state.save()
        }
    }

    private func withLock<T>(_ body: () -> T) -> T {
        cond.lock()
        defer { cond.unlock() }
        return body()
    }

    private func changed() {
        onChange?()
    }
}

private extension Result {
    func unwrapOr(_ fallback: (Failure) -> Success) -> Success {
        switch self {
        case .success(let value): return value
        case .failure(let error): return fallback(error)
        }
    }
}

private func postNotification(summary: String, body: String) {
    let content = UNMutableNotificationContent()
    content.title = summary
    content.body = body
    let request = UNNotificationRequest(identifier: UUID().uuidString, content: content, trigger: nil)
    UNUserNotificationCenter.current().add(request) { _ in }
}
