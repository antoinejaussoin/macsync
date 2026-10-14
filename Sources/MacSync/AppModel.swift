import AppKit
import SwiftUI

final class AppModel: ObservableObject {
    static let shared = AppModel()

    let engine: Engine
    let startHidden: Bool
    private let instance: Instance

    @Published var config: Config
    @Published var state: SyncState
    @Published var running: Running?
    @Published var queue: [Job] = []
    @Published var rsync: RsyncStatus
    @Published var waiting: Set<String> = []
    @Published var statsBusy: Set<String> = []
    @Published var notice: String?
    @Published var nextRuns: [AnchorKey: Date] = [:]
    @Published var now = Date()
    @Published var selectedID: String?
    @Published var draft: Pair?
    @Published var draftIsNew = false
    @Published var confirmDelete = false
    @Published var flash: Flash?
    @Published var customSchedule: Set<String> = []
    @Published var rsyncOverride = ""
    @Published var loginEnabled = false
    @Published var snapshotRefreshToken = 0

    var refreshStatus: ((AppModel) -> Void)?
    private var timer: Timer?

    static var pendingShow = false

    private init() {
        let background = CommandLine.arguments.contains(backgroundArgument)
        guard let instance = Instance.acquire(background: background, onShow: {
            DispatchQueue.main.async { AppModel.requestShow() }
        }) else {
            fputs("MacSync is already running.\n", stderr)
            exit(0)
        }
        self.instance = instance
        let (loaded, notice) = loadConfig()
        let history = SyncState.load()
        let engine = Engine(config: loaded, state: history, notice: notice)
        self.engine = engine
        startHidden = background && !loaded.pairs.isEmpty
        config = loaded
        state = history
        rsync = locateRsync(overridePath: loaded.rsyncPath)
        self.notice = notice
        rsyncOverride = loaded.rsyncPath ?? ""
        loginEnabled = loginItemEnabled()
        selectedID = loaded.pairs.first?.id
        draft = loaded.pairs.first
        engine.setOnChange { [weak self] in
            DispatchQueue.main.async { self?.pull() }
        }
        engine.start()
        pull()
        let timer = Timer(timeInterval: 0.5, repeats: true) { [weak self] _ in
            self?.pull()
        }
        RunLoop.main.add(timer, forMode: .common)
        self.timer = timer
    }

    static func requestShow() {
        pendingShow = true
        NSApp.activate(ignoringOtherApps: true)
        let windows = NSApp.windows.filter { $0.styleMask.contains(.titled) }
        if windows.isEmpty { return }
        pendingShow = false
        for window in windows {
            window.makeKeyAndOrderFront(nil)
        }
    }

    func pull() {
        if !Thread.isMainThread {
            DispatchQueue.main.async { self.pull() }
            return
        }
        let snap = engine.snapshot()
        if config != snap.config { config = snap.config }
        if state != snap.state { state = snap.state }
        if running != snap.running { running = snap.running }
        if queue != snap.queue { queue = snap.queue }
        if rsync != snap.rsync { rsync = snap.rsync }
        if waiting != snap.waiting { waiting = snap.waiting }
        if statsBusy != snap.statsBusy { statsBusy = snap.statsBusy }
        if notice != snap.notice { notice = snap.notice }
        if nextRuns != snap.nextRuns { nextRuns = snap.nextRuns }
        now = Date()
        if let id = selectedID, !draftIsNew, !config.pairs.contains(where: { $0.id == id }) {
            selectedID = nil
            draft = nil
        }
        refreshStatus?(self)
    }

    var isDirty: Bool {
        guard let draft else { return false }
        if draftIsNew { return true }
        return config.pair(draft.id) != draft
    }

    var activeFlash: Flash? {
        guard let flash, now.timeIntervalSince(flash.at) < 8 else { return nil }
        return flash
    }

    func showFlash(_ message: String, error: Bool) {
        flash = Flash(message: message, isError: error, at: Date())
    }

    func select(_ id: String?) {
        if id == selectedID { return }
        if isDirty {
            showFlash("Save or revert your changes first", error: true)
            let keep = selectedID
            DispatchQueue.main.async {
                self.selectedID = keep
            }
            return
        }
        selectedID = id
        draft = id.flatMap { config.pair($0) }
        draftIsNew = false
        confirmDelete = false
    }

    func addPair() {
        if isDirty {
            showFlash("Save or revert your changes first", error: true)
            return
        }
        let pair = Pair.makeNew()
        selectedID = pair.id
        draft = pair
        draftIsNew = true
        confirmDelete = false
    }

    func discardNew() {
        let first = config.pairs.first
        selectedID = first?.id
        draft = first
        draftIsNew = false
    }

    func revertDraft() {
        guard let id = draft?.id, let saved = config.pair(id) else { return }
        draft = saved
    }

    func saveDraft() {
        guard let draft else { return }
        let errors = draft.validate()
        if !errors.isEmpty {
            showFlash(errors.joined(separator: ". "), error: true)
            return
        }
        if !draft.destination.isEmpty, !directoryExists(draft.destination) {
            let parent = URL(fileURLWithPath: draft.destination).deletingLastPathComponent().path
            if directoryExists(parent) {
                do {
                    try FileManager.default.createDirectory(atPath: draft.destination, withIntermediateDirectories: false)
                } catch {
                    showFlash("Could not create \(draft.destination): \(error)", error: true)
                    return
                }
            }
        }
        var updated = config
        if let index = updated.pairs.firstIndex(where: { $0.id == draft.id }) {
            updated.pairs[index] = draft
        } else {
            updated.pairs.append(draft)
        }
        do {
            try engine.replaceConfig(updated)
            draftIsNew = false
            showFlash("Saved \"\(draft.displayName)\"", error: false)
            snapshotRefreshToken += 1
            pull()
        } catch {
            showFlash(error.localizedDescription, error: true)
        }
    }

    func deleteSelected() {
        guard let id = selectedID else { return }
        engine.cancel(pairID: id)
        var updated = config
        updated.pairs.removeAll { $0.id == id }
        do {
            try engine.replaceConfig(updated)
            engine.removePairState(id)
            pull()
            let next = config.pairs.first
            selectedID = next?.id
            draft = next
            draftIsNew = false
            confirmDelete = false
            showFlash("Folder pair removed. Its snapshots were left on disk.", error: false)
        } catch {
            showFlash(error.localizedDescription, error: true)
        }
    }

    func setLoginEnabled(_ enabled: Bool) {
        do {
            if enabled { try enableLoginItem() } else { try disableLoginItem() }
            loginEnabled = enabled
        } catch {
            showFlash(error.localizedDescription, error: true)
        }
    }

    func applyRsyncOverride() {
        var updated = config
        let value = rsyncOverride.trimmingCharacters(in: .whitespacesAndNewlines)
        updated.rsyncPath = value.isEmpty ? nil : value
        do {
            try engine.replaceConfig(updated)
            pull()
        } catch {
            showFlash(error.localizedDescription, error: true)
        }
    }

    func chooseSource() {
        guard let path = chooseFolder(title: "Folder to back up"), var draft else { return }
        if draft.name == Pair.makeNew().name || draft.name.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty {
            let folder = URL(fileURLWithPath: path).lastPathComponent
            if !folder.isEmpty { draft.name = folder }
        }
        draft.source = path
        self.draft = draft
    }

    func chooseDestination() {
        guard let path = chooseFolder(title: "Where to store snapshots"), var draft else { return }
        draft.destination = suggestedDestination(picked: path, source: draft.source)
        self.draft = draft
    }

    func enqueue(_ id: String, _ kind: JobKind) {
        engine.enqueue(pairID: id, kind: kind)
    }

    func enqueueAll() {
        engine.enqueueAllBackups()
    }

    func stop(_ id: String) {
        engine.cancel(pairID: id)
    }

    func refreshRsync() {
        engine.refreshRsync()
    }

    func refreshStats() {
        guard let id = draft?.id, !draftIsNew else { return }
        if running != nil || !queue.isEmpty {
            showFlash("Disk usage is measured only while nothing is copying.", error: false)
            return
        }
        engine.refreshStats(pairID: id)
    }

    func dismissNotice() {
        engine.dismissNotice()
    }

    func shutdown() {
        engine.shutdown()
    }

    func draftBinding<T>(_ keyPath: WritableKeyPath<Pair, T>, fallback: T) -> Binding<T> {
        Binding(
            get: { self.draft?[keyPath: keyPath] ?? fallback },
            set: { self.draft?[keyPath: keyPath] = $0 }
        )
    }
}

struct Flash: Equatable {
    var message: String
    var isError: Bool
    var at: Date
}

private func directoryExists(_ path: String) -> Bool {
    var isDir: ObjCBool = false
    return FileManager.default.fileExists(atPath: path, isDirectory: &isDir) && isDir.boolValue
}

private func chooseFolder(title: String) -> String? {
    let panel = NSOpenPanel()
    panel.canChooseFiles = false
    panel.canChooseDirectories = true
    panel.canCreateDirectories = true
    panel.prompt = "Choose"
    panel.title = title
    guard panel.runModal() == .OK else { return nil }
    return panel.url?.path
}
