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
            VStack(alignment: .leading, spacing: 8) {
                if case .missing(let message) = model.rsync {
                    banner(icon: "exclamationmark.triangle.fill", tint: .red) {
                        VStack(alignment: .leading, spacing: 2) {
                            Text("rsync unavailable").font(.headline)
                            Text(message).font(.callout).foregroundStyle(.secondary).fixedSize(horizontal: false, vertical: true)
                        }
                        Spacer(minLength: 8)
                        Button("Check again") { model.refreshRsync() }
                    }
                }
                if let notice = model.notice {
                    banner(icon: "info.circle.fill", tint: .orange) {
                        Text(notice).fixedSize(horizontal: false, vertical: true)
                        Spacer(minLength: 8)
                        Button("Dismiss") { model.dismissNotice() }
                    }
                }
                if let flash = model.activeFlash {
                    banner(icon: flash.isError ? "xmark.circle.fill" : "checkmark.circle.fill", tint: flash.isError ? .red : .green) {
                        Text(flash.message).fixedSize(horizontal: false, vertical: true)
                    }
                }
            }
            .padding(12)
        }
    }

    private func banner<Content: View>(icon: String, tint: Color, @ViewBuilder content: () -> Content) -> some View {
        HStack(alignment: .center, spacing: 10) {
            Image(systemName: icon).foregroundStyle(tint).font(.title3)
            content()
        }
        .padding(12)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(tint.opacity(0.12), in: RoundedRectangle(cornerRadius: 10, style: .continuous))
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
                    .frame(maxWidth: .infinity)
            }
            .buttonStyle(.bordered)
            .controlSize(.regular)
            .padding(.horizontal, 12)
            .padding(.bottom, 8)
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
                Label("rsync \(info.version)", systemImage: "checkmark.circle.fill")
                    .font(.caption)
                    .foregroundStyle(.secondary)
                    .help(info.path)
            case .missing:
                Label("rsync was not found", systemImage: "exclamationmark.circle.fill")
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
        let status = override.map { ($0.0, $0.1, "circle.dashed") } ?? pairStatus(pair)
        HStack(alignment: .center, spacing: 10) {
            Image(systemName: status.2)
                .font(.system(size: 11, weight: .bold))
                .foregroundStyle(.white)
                .frame(width: 22, height: 22)
                .background(status.1, in: Circle())
            VStack(alignment: .leading, spacing: 2) {
                Text(pair.displayName).font(.body.weight(.medium)).lineLimit(1)
                Text(status.0)
                    .font(.caption)
                    .foregroundStyle(.secondary)
                    .lineLimit(1)
            }
        }
        .padding(.vertical, 4)
    }

    private func pairStatus(_ pair: Pair) -> (String, Color, String) {
        if let running = model.running, running.job.pairID == pair.id {
            let text: String
            if running.job.kind == .backup, let progress = running.progress, !progress.scanning {
                text = "Backing up · \(progress.percent)%"
            } else {
                text = "\(running.job.kind.verb)…"
            }
            return (text, .blue, "arrow.triangle.2.circlepath")
        }
        if model.queue.contains(where: { $0.pairID == pair.id }) {
            return ("Queued", .blue, "clock")
        }
        if model.waiting.contains(pair.id) {
            return ("Waiting for destination drive", .orange, "externaldrive.badge.exclamationmark")
        }
        if !pair.enabled { return ("Paused", .secondary, "pause.fill") }
        guard let backup = model.state.pairs[pair.id]?.lastBackup else {
            return ("Never backed up", .secondary, "circle")
        }
        if backup.outcome == .failed {
            return ("Failed \(relativeTime(backup.finished, now: model.now))", .red, "exclamationmark")
        }
        let symbol = backup.outcome == .warning ? "exclamationmark" : "checkmark"
        return ("Backed up \(relativeTime(backup.finished, now: model.now))", outcomeColor(backup.outcome), symbol)
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
            VStack(alignment: .leading, spacing: 22) {
                HStack(alignment: .center, spacing: 16) {
                    Image(systemName: "clock.arrow.circlepath")
                        .font(.system(size: 28, weight: .medium))
                        .foregroundStyle(.white)
                        .frame(width: 64, height: 64)
                        .background(Color.accentColor, in: RoundedRectangle(cornerRadius: 16, style: .continuous))
                    VStack(alignment: .leading, spacing: 4) {
                        Text("Dated snapshots of your folders")
                            .font(.title.weight(.semibold))
                        Text("Each run is a complete, browsable copy. Unchanged files are hard-linked, so a snapshot only costs the space of what changed.")
                            .foregroundStyle(.secondary)
                            .fixedSize(horizontal: false, vertical: true)
                    }
                }
                step(1, "rsync", "shippingbox") {
                    switch model.rsync {
                    case .found(let info):
                        Label("Found rsync \(info.version)", systemImage: "checkmark.circle.fill")
                            .foregroundStyle(.green)
                            .help(info.path)
                    case .missing:
                        Text("Not found. MacSync.app includes rsync. This unpackaged build needs Homebrew: brew install rsync")
                            .foregroundStyle(.red)
                    }
                }
                step(2, "Full Disk Access", "lock.shield") {
                    Text("Desktop, Documents and other protected folders can only be read once MacSync is allowed in System Settings.")
                        .foregroundStyle(.secondary)
                        .fixedSize(horizontal: false, vertical: true)
                    Button("Open Full Disk Access settings") {
                        if let url = URL(string: fullDiskAccessURL) {
                            NSWorkspace.shared.open(url)
                        }
                    }
                }
                step(3, "Add a folder pair", "folder.badge.plus") {
                    Text("Pick a folder to protect and a folder on another drive for its snapshots. APFS or Mac OS Extended works; exFAT has no hard links.")
                        .foregroundStyle(.secondary)
                        .fixedSize(horizontal: false, vertical: true)
                    Button { model.addPair() } label: {
                        Label("Add folder pair", systemImage: "plus")
                    }
                    .buttonStyle(.borderedProminent)
                }
            }
            .padding(28)
            .frame(maxWidth: 640, alignment: .leading)
            .frame(maxWidth: .infinity)
        }
    }

    private func step<Content: View>(_ number: Int, _ title: String, _ icon: String, @ViewBuilder content: () -> Content) -> some View {
        HStack(alignment: .top, spacing: 14) {
            Text("\(number)")
                .font(.callout.weight(.semibold))
                .foregroundStyle(.white)
                .frame(width: 26, height: 26)
                .background(Color.accentColor.opacity(0.9), in: Circle())
            VStack(alignment: .leading, spacing: 8) {
                Label(title, systemImage: icon).font(.headline)
                content()
            }
            .frame(maxWidth: .infinity, alignment: .leading)
        }
        .padding(14)
        .background {
            RoundedRectangle(cornerRadius: 12, style: .continuous)
                .fill(Color(nsColor: .controlBackgroundColor))
        }
        .overlay {
            RoundedRectangle(cornerRadius: 12, style: .continuous)
                .strokeBorder(Color.primary.opacity(0.06))
        }
    }
}
