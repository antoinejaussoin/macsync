import AppKit
import SwiftUI

enum AppInfo {
    static let version = "0.1.0"
}

@main
struct MacSyncApp: App {
    @NSApplicationDelegateAdaptor(AppDelegate.self) var appDelegate
    @StateObject private var model = AppModel.shared

    var body: some Scene {
        Window("MacSync", id: "main") {
            ContentView()
                .environmentObject(model)
                .frame(minWidth: 760, minHeight: 520)
        }
        .commands {
            CommandGroup(replacing: .newItem) {
                Button("Add Folder Pair") { model.addPair() }
                    .keyboardShortcut("n", modifiers: .command)
            }
        }
    }
}

final class AppDelegate: NSObject, NSApplicationDelegate {
    private let status = StatusMenu()

    func applicationDidFinishLaunching(_ notification: Notification) {
        NSApp.setActivationPolicy(.accessory)
        let model = AppModel.shared
        status.install()
        model.refreshStatus = { [weak status] model in
            status?.update(model)
        }
        status.update(model)
        if model.startHidden, !AppModel.pendingShow {
            DispatchQueue.main.async {
                NSApp.windows.forEach { $0.orderOut(nil) }
            }
        } else {
            NSApp.activate(ignoringOtherApps: true)
        }
    }

    func applicationShouldTerminate(_ sender: NSApplication) -> NSApplication.TerminateReply {
        AppModel.shared.shutdown()
        return .terminateNow
    }

    func applicationShouldTerminateAfterLastWindowClosed(_ sender: NSApplication) -> Bool {
        false
    }
}

final class StatusMenu: NSObject {
    private var statusItem: NSStatusItem?
    private var statusMenuItem = NSMenuItem(title: "", action: nil, keyEquivalent: "")
    private var pairItems: [(String, NSMenuItem)] = []
    private var layout: [(String, String)] = []
    private var alert = false

    func install() {
        let item = NSStatusBar.system.statusItem(withLength: NSStatusItem.variableLength)
        item.button?.image = menuIcon(alert: false)
        item.button?.toolTip = "MacSync"
        statusItem = item
        rebuild([])
    }

    func update(_ model: AppModel) {
        guard let statusItem else { return }
        let nextLayout = model.config.pairs.map { ($0.id, $0.displayName) }
        if nextLayout.map(\.0) != layout.map(\.0) || nextLayout.map(\.1) != layout.map(\.1) {
            rebuild(nextLayout)
        }
        let (status, title) = statusLine(model)
        statusMenuItem.title = status
        statusItem.button?.title = title ?? ""
        statusItem.button?.toolTip = "MacSync: \(status)"
        for (id, item) in pairItems {
            if let pair = model.config.pair(id) {
                item.title = "\(pair.displayName)  —  \(pairSummary(model, id: id))"
            }
        }
        let failed = model.config.pairs.contains { pair in
            guard let record = model.state.pairs[pair.id] else { return false }
            return record.lastBackup?.outcome == .failed || record.lastIntegrity?.outcome == .failed
        }
        let nextAlert = failed || model.rsync.message != nil
        if nextAlert != alert {
            alert = nextAlert
            statusItem.button?.image = menuIcon(alert: alert)
        }
    }

    private func rebuild(_ nextLayout: [(String, String)]) {
        let menu = NSMenu()
        statusMenuItem = NSMenuItem(title: "", action: nil, keyEquivalent: "")
        statusMenuItem.isEnabled = false
        menu.addItem(statusMenuItem)
        menu.addItem(.separator())
        pairItems = nextLayout.map { id, name in
            let item = NSMenuItem(title: name, action: #selector(backupPair(_:)), keyEquivalent: "")
            item.target = self
            item.representedObject = id
            menu.addItem(item)
            return (id, item)
        }
        if !nextLayout.isEmpty { menu.addItem(.separator()) }
        let all = NSMenuItem(title: "Back Up All Now", action: #selector(backupAll), keyEquivalent: "")
        all.target = self
        all.isEnabled = !nextLayout.isEmpty
        menu.addItem(all)
        let open = NSMenuItem(title: "Open MacSync…", action: #selector(openWindow), keyEquivalent: "")
        open.target = self
        menu.addItem(open)
        menu.addItem(.separator())
        let quit = NSMenuItem(title: "Quit MacSync", action: #selector(quit), keyEquivalent: "q")
        quit.target = self
        menu.addItem(quit)
        statusItem?.menu = menu
        layout = nextLayout
    }

    private func statusLine(_ model: AppModel) -> (String, String?) {
        if case .missing(let message) = model.rsync {
            let first = message.split(whereSeparator: \.isNewline).first.map(String.init) ?? "rsync missing"
            return (first, nil)
        }
        if let running = model.running {
            let text: String
            let title: String?
            if running.job.kind == .backup, let progress = running.progress, let remaining = aboutRemaining(progress.eta) {
                text = "Backing up \(running.pairName) (\(remaining))"
                title = shortRemaining(progress.eta)
            } else {
                text = "\(running.job.kind.verb) \(running.pairName)…"
                title = nil
            }
            return (text, title)
        }
        if !model.queue.isEmpty {
            return ("\(model.queue.count) job(s) queued", nil)
        }
        let newest = model.state.pairs.values.compactMap(\.lastSuccess).max()
        if let newest {
            return ("Idle · last backup \(relativeTime(newest, now: model.now))", nil)
        }
        if model.config.pairs.isEmpty {
            return ("No folders set up yet", nil)
        }
        return ("Idle · no backups yet", nil)
    }

    private func pairSummary(_ model: AppModel, id: String) -> String {
        if model.running?.job.pairID == id { return "running" }
        if model.queue.contains(where: { $0.pairID == id }) { return "queued" }
        if model.waiting.contains(id) { return "drive not connected" }
        guard let backup = model.state.pairs[id]?.lastBackup else { return "never backed up" }
        if backup.outcome == .failed {
            return "failed \(relativeTime(backup.finished, now: model.now))"
        }
        return relativeTime(backup.finished, now: model.now)
    }

    private func menuIcon(alert: Bool) -> NSImage {
        let name = alert ? "clock.badge.exclamationmark" : "clock.arrow.circlepath"
        let image = NSImage(systemSymbolName: name, accessibilityDescription: "MacSync")
            ?? NSImage(systemSymbolName: "clock.arrow.circlepath", accessibilityDescription: "MacSync")
            ?? NSImage()
        image.isTemplate = true
        return image
    }

    @objc private func openWindow() { AppModel.requestShow() }
    @objc private func backupAll() { AppModel.shared.enqueueAll() }
    @objc private func quit() { NSApp.terminate(nil) }
    @objc private func backupPair(_ sender: NSMenuItem) {
        guard let id = sender.representedObject as? String else { return }
        AppModel.shared.enqueue(id, .backup)
    }
}

struct WindowChrome: NSViewRepresentable {
    var startHidden: Bool

    func makeCoordinator() -> Coordinator { Coordinator(startHidden: startHidden) }

    func makeNSView(context: Context) -> NSView {
        let view = NSView(frame: .zero)
        DispatchQueue.main.async { context.coordinator.attach(view) }
        return view
    }

    func updateNSView(_ nsView: NSView, context: Context) {
        DispatchQueue.main.async { context.coordinator.attach(nsView) }
    }

    final class Coordinator: NSObject, NSWindowDelegate {
        var startHidden: Bool
        var applied = false
        weak var original: NSWindowDelegate?
        weak var window: NSWindow?

        init(startHidden: Bool) { self.startHidden = startHidden }

        func attach(_ view: NSView) {
            guard let window = view.window, window !== self.window else { return }
            self.window = window
            if window.delegate !== self {
                original = window.delegate
                window.delegate = self
            }
            if window.frame.width < 900 || window.frame.height < 600 {
                window.setContentSize(NSSize(width: 980, height: 720))
            }
            window.minSize = NSSize(width: 760, height: 520)
            if startHidden, !applied, !AppModel.pendingShow {
                applied = true
                window.orderOut(nil)
            }
            if AppModel.pendingShow {
                AppModel.requestShow()
            }
        }

        func windowShouldClose(_ sender: NSWindow) -> Bool {
            sender.orderOut(nil)
            return false
        }

        override func responds(to selector: Selector!) -> Bool {
            if super.responds(to: selector) { return true }
            return original?.responds(to: selector) ?? false
        }

        override func forwardingTarget(for selector: Selector!) -> Any? {
            if super.responds(to: selector) { return nil }
            return original
        }
    }
}
