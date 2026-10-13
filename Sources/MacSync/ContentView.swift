import AppKit
import SwiftUI

private let fullDiskAccessURL = "x-apple.systempreferences:com.apple.preference.security?Privacy_AllFiles"

struct ContentView: View {
    @EnvironmentObject private var model: AppModel

    var body: some View {
        VStack(spacing: 0) {
            banners
            NavigationSplitView {
                sidebar
                    .navigationSplitViewColumnWidth(min: 220, ideal: 260, max: 340)
            } detail: {
                if model.draft != nil {
                    DetailView()
                } else {
                    WelcomeView()
                }
            }
        }
        .background(WindowChrome(startHidden: model.startHidden))
        .navigationTitle("MacSync")
    }

    @ViewBuilder private var banners: some View {
        if model.rsync.message != nil || model.notice != nil || model.activeFlash != nil {
            VStack(alignment: .leading, spacing: 6) {
                if case .missing(let message) = model.rsync {
                    HStack(alignment: .firstTextBaseline) {
                        Label("rsync unavailable", systemImage: "exclamationmark.triangle.fill")
                            .foregroundStyle(.red)
                        Text(message).fixedSize(horizontal: false, vertical: true)
                        Spacer(minLength: 8)
                        Button("Check again") { model.refreshRsync() }
                    }
                }
                if let notice = model.notice {
                    HStack(alignment: .firstTextBaseline) {
                        Text(notice).foregroundStyle(.orange).fixedSize(horizontal: false, vertical: true)
                        Spacer(minLength: 8)
                        Button("Dismiss") { model.dismissNotice() }
                    }
                }
                if let flash = model.activeFlash {
                    Text(flash.message)
                        .foregroundStyle(flash.isError ? Color.red : Color.green)
                        .fixedSize(horizontal: false, vertical: true)
                }
            }
            .padding(10)
            .frame(maxWidth: .infinity, alignment: .leading)
            .background(.quaternary.opacity(0.35))
        }
    }

    private var sidebar: some View {
        VStack(spacing: 0) {
            List(selection: Binding(get: { model.selectedID }, set: { model.select($0) })) {
                ForEach(model.config.pairs) { pair in
                    PairRow(pair: pair).tag(pair.id)
                }
                if model.draftIsNew, let draft = model.draft {
                    PairRow(pair: draft, override: ("Not saved yet", .secondary)).tag(draft.id)
                }
            }
            .listStyle(.sidebar)
            Button {
                model.addPair()
            } label: {
                Label("Add folder pair", systemImage: "plus")
                    .frame(maxWidth: .infinity, alignment: .leading)
            }
            .buttonStyle(.plain)
            .padding(.horizontal, 12)
            .padding(.vertical, 8)
        }
        .safeAreaInset(edge: .bottom, spacing: 0) { settings }
        .navigationTitle("MacSync")
    }

    private var settings: some View {
        VStack(alignment: .leading, spacing: 8) {
            Divider()
            Toggle("Launch at login", isOn: Binding(get: { model.loginEnabled }, set: { model.setLoginEnabled($0) }))
            if model.loginEnabled, !runningFromBundle() {
                Text("Running outside MacSync.app: the login item points at this build.")
                    .font(.caption)
                    .foregroundStyle(.orange)
                    .fixedSize(horizontal: false, vertical: true)
            }
            switch model.rsync {
            case .found(let info):
                Text("rsync \(info.version) · \(info.path)")
                    .font(.caption)
                    .foregroundStyle(.secondary)
                    .textSelection(.enabled)
            case .missing:
                Text("rsync was not found. The app includes its own copy.")
                    .font(.caption)
                    .foregroundStyle(.red)
            }
            DisclosureGroup("rsync location") {
                TextField("Leave empty for the included rsync", text: $model.rsyncOverride)
                    .textFieldStyle(.roundedBorder)
                Button("Apply") { model.applyRsyncOverride() }
            }
            .font(.caption)
        }
        .padding(12)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(.bar)
    }
}

struct PairRow: View {
    @EnvironmentObject private var model: AppModel
    var pair: Pair
    var override: (String, Color)?

    var body: some View {
        let status = override ?? pairStatus(pair)
        HStack(alignment: .center, spacing: 8) {
            Circle().fill(status.1).frame(width: 8, height: 8)
            VStack(alignment: .leading, spacing: 1) {
                Text(pair.displayName).lineLimit(1)
                Text(status.0)
                    .font(.caption)
                    .foregroundStyle(.secondary)
                    .lineLimit(1)
            }
        }
        .padding(.vertical, 2)
    }

    private func pairStatus(_ pair: Pair) -> (String, Color) {
        if let running = model.running, running.job.pairID == pair.id {
            let text: String
            if running.job.kind == .backup, let progress = running.progress, !progress.scanning {
                text = "Backing up · \(progress.percent)%"
            } else {
                text = "\(running.job.kind.verb)…"
            }
            return (text, .blue)
        }
        if model.queue.contains(where: { $0.pairID == pair.id }) {
            return ("Queued", .blue)
        }
        if model.waiting.contains(pair.id) {
            return ("Waiting for destination drive", .orange)
        }
        if !pair.enabled { return ("Paused", .secondary) }
        guard let backup = model.state.pairs[pair.id]?.lastBackup else {
            return ("Never backed up", .secondary)
        }
        if backup.outcome == .failed {
            return ("Failed \(relativeTime(backup.finished, now: model.now))", .red)
        }
        return ("Backed up \(relativeTime(backup.finished, now: model.now))", outcomeColor(backup.outcome))
    }
}

func outcomeColor(_ outcome: Outcome) -> Color {
    switch outcome {
    case .success: return .green
    case .warning: return .orange
    case .failed: return .red
    }
}

struct WelcomeView: View {
    @EnvironmentObject private var model: AppModel

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 16) {
                Text("Dated snapshots of your folders")
                    .font(.title2.bold())
                Text("MacSync copies each source folder into a new dated snapshot on a schedule. Unchanged files are hard-linked to the previous snapshot, so every snapshot is a complete, browsable copy that only costs the space of what changed.")
                    .foregroundStyle(.secondary)
                    .fixedSize(horizontal: false, vertical: true)
                step("1. rsync") {
                    switch model.rsync {
                    case .found(let info):
                        Text("Found rsync \(info.version) at \(info.path)").foregroundStyle(.green)
                    case .missing:
                        Text("Not found. MacSync.app includes rsync. This unpackaged build needs Homebrew: brew install rsync")
                            .foregroundStyle(.red)
                    }
                }
                step("2. Full Disk Access") {
                    Text("Desktop, Documents and other protected folders can only be read once MacSync is allowed in System Settings > Privacy & Security > Full Disk Access.")
                        .fixedSize(horizontal: false, vertical: true)
                    Button("Open Full Disk Access settings") {
                        if let url = URL(string: fullDiskAccessURL) {
                            NSWorkspace.shared.open(url)
                        }
                    }
                }
                step("3. Add a folder pair") {
                    Text("Pick a folder to protect and a folder on another drive to hold its snapshots. APFS or Mac OS Extended drives work; exFAT has no hard links.")
                        .fixedSize(horizontal: false, vertical: true)
                    Button("Add folder pair", systemImage: "plus") { model.addPair() }
                }
            }
            .padding(24)
            .frame(maxWidth: 640, alignment: .leading)
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
    }

    private func step<Content: View>(_ title: String, @ViewBuilder content: () -> Content) -> some View {
        VStack(alignment: .leading, spacing: 6) {
            Text(title).font(.headline)
            content()
        }
    }
}
