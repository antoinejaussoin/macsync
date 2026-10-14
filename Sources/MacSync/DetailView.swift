import AppKit
import SwiftUI

struct BackupProgressBar: NSViewRepresentable {
    /// Nil while rsync is still discovering files.
    var fraction: Double?

    func makeNSView(context: Context) -> NSProgressIndicator {
        let bar = NSProgressIndicator()
        bar.style = .bar
        bar.controlSize = .regular
        bar.minValue = 0
        bar.maxValue = 1
        bar.usesThreadedAnimation = true
        bar.setContentHuggingPriority(.defaultLow, for: .horizontal)
        bar.setContentCompressionResistancePriority(.defaultLow, for: .horizontal)
        return bar
    }

    func updateNSView(_ bar: NSProgressIndicator, context: Context) {
        if let fraction {
            if bar.isIndeterminate { bar.stopAnimation(nil) }
            bar.isIndeterminate = false
            bar.doubleValue = min(1, max(0, fraction))
        } else if !bar.isIndeterminate || !bar.isDisplayedWhenStopped {
            bar.isIndeterminate = true
            bar.startAnimation(nil)
        }
    }
}

struct DetailView: View {
    @EnvironmentObject private var model: AppModel

    private var draft: Pair { model.draft! }
    private var errors: [String] { draft.validate() }
    private var dirty: Bool { model.isDirty }

    var body: some View {
        if model.draft != nil {
            editor
        }
    }

    private var editor: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 16) {
                header
                actions
                statusSection
                foldersSection
                sizesSection
                scheduleSection
                retentionSection
                optionsSection
                if let record = model.state.pairs[draft.id]?.lastIntegrity, !model.draftIsNew {
                    integritySection(record)
                }
                if !model.draftIsNew { logSection }
                protectionSection
                if !model.draftIsNew { deleteSection }
            }
            .padding(24)
            .frame(maxWidth: 860)
            .frame(maxWidth: .infinity)
        }
        .safeAreaInset(edge: .bottom, spacing: 0) { saveBar }
    }

    private var header: some View {
        HStack(alignment: .center, spacing: 16) {
            TextField("Name", text: model.draftBinding(\.name, fallback: ""))
                .textFieldStyle(.plain)
                .font(.largeTitle.weight(.semibold))
            Spacer(minLength: 12)
            Toggle("Run on schedule", isOn: model.draftBinding(\.enabled, fallback: true))
                .toggleStyle(.switch)
                .fixedSize()
        }
    }

    @ViewBuilder private var actions: some View {
        if model.draftIsNew {
            Label("Choose the folders below, then save to start backing up.", systemImage: "arrow.down")
                .foregroundStyle(.secondary)
                .padding(14)
                .frame(maxWidth: .infinity, alignment: .leading)
                .macCard()
        } else if let running = model.running, running.job.pairID == draft.id {
            VStack(alignment: .leading, spacing: 6) {
                if running.job.kind == .backup, let progress = running.progress {
                    if progress.scanning {
                        BackupProgressBar(fraction: nil)
                            .frame(maxWidth: .infinity)
                            .frame(height: 16)
                        Text("Scanning files… \(progress.filesTotal.map { "\($0) found" } ?? "looking for changes")")
                            .font(.callout)
                    } else {
                        BackupProgressBar(fraction: Double(progress.percent) / 100)
                            .frame(maxWidth: .infinity)
                            .frame(height: 16)
                        Text("\(progress.percent)%  ·  \(formatBytes(progress.bytes)) copied  ·  \(progress.rate)  ·  \(progress.eta) left")
                            .font(.callout)
                        if let checked = progress.filesChecked, let total = progress.filesTotal {
                            Text("\(checked) of \(total) files checked, \(progress.filesTransferred) copied")
                                .font(.caption)
                                .foregroundStyle(.secondary)
                        }
                    }
                } else {
                    HStack {
                        ProgressView().controlSize(.small)
                        Text("\(running.job.kind.verb) since \(SelfDate.time.string(from: running.started))")
                    }
                }
                Button("Stop", role: .destructive) { model.stop(draft.id) }
                    .buttonStyle(.bordered)
            }
            .padding(14)
            .macCard()
        } else {
            let rsyncOK = model.rsync.info != nil
            let record = model.state.pairs[draft.id]
            let hasSnapshot = record?.lastSuccess != nil || latestSnapshotName(dest: URL(fileURLWithPath: draft.destination)) != nil
            HStack(spacing: 8) {
                let backupQueued = model.queue.contains { $0.pairID == draft.id && $0.kind == .backup }
                Button { model.enqueue(draft.id, .backup) } label: {
                    Label(backupQueued ? "Queued…" : "Back up now", systemImage: "arrow.clockwise")
                }
                .buttonStyle(.borderedProminent)
                .disabled(!rsyncOK || dirty || backupQueued)
                let checkQueued = model.queue.contains { $0.pairID == draft.id && $0.kind == .integrity }
                Button { model.enqueue(draft.id, .integrity) } label: {
                    Label(checkQueued ? "Queued…" : "Check integrity", systemImage: "checkmark.shield")
                }
                .buttonStyle(.bordered)
                .disabled(!rsyncOK || dirty || checkQueued || !hasSnapshot)
                .help("Compares every file in the source with the latest snapshot by checksum. Nothing is written.")
                Button { showSnapshots() } label: {
                    Label("Show snapshots", systemImage: "folder")
                }
                .buttonStyle(.bordered)
                    .disabled(!directoryExists(draft.destination))
                if dirty {
                    Text("Save changes to run").font(.caption).foregroundStyle(.secondary)
                }
                Spacer(minLength: 0)
            }
        }
    }

    private var statusSection: some View {
        VStack(alignment: .leading, spacing: 8) {
            statusRow("Last backup") {
                if let backup = model.state.pairs[draft.id]?.lastBackup {
                    Text(relativeTime(backup.finished, now: model.now) + " (\(SelfDate.weekday.string(from: backup.finished)))")
                        .foregroundStyle(outcomeColor(backup.outcome))
                    Text(backup.message).foregroundStyle(.secondary).textSelection(.enabled)
                } else {
                    Text("Never").foregroundStyle(.secondary)
                }
            }
            if let success = model.state.pairs[draft.id]?.lastSuccess,
               model.state.pairs[draft.id]?.lastBackup?.outcome == .failed
            {
                statusRow("Last success") {
                    Text(relativeTime(success, now: model.now))
                }
            }
            if !model.draftIsNew {
                statusRow("Next backup") { Text(nextText(.backup)) }
                statusRow("Next check") { Text(nextText(.integrity)) }
            }
        }
        .padding(14)
        .macCard()
    }

    private var foldersSection: some View {
        section("Folders", icon: "folder") {
            folderRow("Source", path: draft.source, placeholder: "Folder to protect") { model.chooseSource() }
            folderRow("Destination", path: draft.destination, placeholder: "Where snapshots are stored") { model.chooseDestination() }
            folderWarnings
        }
    }

    @ViewBuilder private var folderWarnings: some View {
        if !draft.destination.isEmpty {
            switch checkPairPaths(source: draft.source, destination: draft.destination) {
            case .failure(.destinationMissing) where directoryExists(URL(fileURLWithPath: draft.destination).deletingLastPathComponent().path):
                Text("This folder will be created when you save.")
                    .font(.caption)
                    .foregroundStyle(.secondary)
            case .failure(let error):
                switch error {
                case .sourceMissing, .destinationMissing:
                    Text(error.description).foregroundStyle(.orange)
                default:
                    EmptyView()
                }
            default:
                EmptyView()
            }
            if let info = filesystemInfo(path: draft.destination), !filesystemSupportsHardLinks(info.fsType) {
                Text("This drive is formatted as \(info.fsType), which has no hard links. Use APFS or Mac OS Extended.")
                    .foregroundStyle(.red)
                    .fixedSize(horizontal: false, vertical: true)
            }
        }
    }

    private var sizesSection: some View {
        section("Sizes", icon: "internaldrive") {
            let stats = model.state.pairs[draft.id]?.stats ?? .empty
            let measuring = model.statsBusy.contains(draft.id)
            let snapshotDetail: String = {
                if let oldest = stats.oldest, let newest = stats.newest {
                    return "\(friendlyStamp(oldest)) – \(friendlyStamp(newest))"
                }
                return stats.snapshotCount == 0 ? "None yet" : "On disk"
            }()
            LazyVGrid(columns: [GridItem(.flexible(), spacing: 10), GridItem(.flexible(), spacing: 10)], spacing: 10) {
                MetricTile(title: "Source", value: bytes(stats.sourceBytes), detail: "Live folder")
                MetricTile(title: "Snapshots", value: bytes(stats.destinationBytes), detail: "Unchanged files stored once")
                MetricTile(title: "Count", value: "\(stats.snapshotCount)", detail: snapshotDetail)
                MetricTile(
                    title: "Free space",
                    value: stats.freeBytes.map(formatBytes) ?? "—",
                    detail: stats.filesystem.map { $0.uppercased() } ?? "Destination drive"
                )
            }
            HStack {
                if measuring {
                    ProgressView().controlSize(.small)
                    Text("Measuring…").font(.caption).foregroundStyle(.secondary)
                } else {
                    if let measured = stats.measured {
                        Text("Measured \(relativeTime(measured, now: model.now))")
                            .font(.caption)
                            .foregroundStyle(.secondary)
                    }
                    if !model.draftIsNew {
                        Button("Refresh") { model.refreshStats() }
                            .controlSize(.small)
                    }
                }
            }
            if let error = stats.error {
                Text(error).font(.caption).foregroundStyle(.orange)
            }
        }
    }

    private var scheduleSection: some View {
        section("Schedule", icon: "calendar") {
            scheduleRow("Back up", which: "backup", binding: model.draftBinding(\.backupSchedule, fallback: ""))
            scheduleRow("Check integrity", which: "integrity", binding: model.draftBinding(\.integritySchedule, fallback: ""))
            Text("If the Mac is asleep or the drive is unplugged at the scheduled time, the job runs once when it can.")
                .font(.caption)
                .foregroundStyle(.secondary)
                .fixedSize(horizontal: false, vertical: true)
        }
    }

    private var retentionSection: some View {
        section("Retention", icon: "clock.arrow.circlepath") {
            retentionRow("Keep the last", value: model.draftBinding(\.retention.keepLast, fallback: 30), range: 0 ... 10_000, unit: "snapshots")
            retentionRow("One per day for", value: model.draftBinding(\.retention.keepDaily, fallback: 0), range: 0 ... 3650, unit: "days")
            retentionRow("One per week for", value: model.draftBinding(\.retention.keepWeekly, fallback: 0), range: 0 ... 520, unit: "weeks")
            retentionRow("One per month for", value: model.draftBinding(\.retention.keepMonthly, fallback: 3), range: 0 ... 1200, unit: "months")
            HStack {
                Text("One per year").frame(width: 140, alignment: .leading)
                let forever = draft.retention.keepYearly == .forever
                Toggle("forever", isOn: Binding(
                    get: { forever },
                    set: { on in
                        model.draft?.retention.keepYearly = on ? .forever : .count(5)
                    }
                ))
                if case .count(let count) = draft.retention.keepYearly {
                    NumberField(
                        value: Binding(
                            get: { count },
                            set: { model.draft?.retention.keepYearly = .count($0) }
                        ),
                        range: 0 ... 100
                    )
                    Text("years").foregroundStyle(.secondary)
                }
            }
            Text("0 turns a tier off. Each tier keeps the newest snapshot of each day, week, month or year; a period already covered by another tier is not stored twice. The snapshot \"latest\" points at is never removed.")
                .font(.caption)
                .foregroundStyle(.secondary)
                .fixedSize(horizontal: false, vertical: true)
            RetentionPreview(destination: draft.destination, policy: draft.retention, token: model.snapshotRefreshToken)
        }
    }

    private var optionsSection: some View {
        section("Options", icon: "slider.horizontal.3") {
            Toggle("Make snapshots read-only", isOn: model.draftBinding(\.readOnly, fallback: true))
                .help("Files are copied without write permission and snapshot folders are locked after each backup. Restored files come back read-only. Changing this setting makes the next backup copy every file once.")
            Text("Exclude (one rsync pattern per line)")
            ExcludesEditor(pairID: draft.id, excludes: model.draftBinding(\.excludes, fallback: []))
        }
    }

    private func integritySection(_ record: IntegrityRecord) -> some View {
        section("Last integrity check", icon: "checkmark.shield") {
            HStack(alignment: .firstTextBaseline, spacing: 6) {
                if let finished = record.finished {
                    Text("\(relativeTime(finished, now: model.now)):")
                }
                Text(record.message).foregroundStyle(record.outcome.map(outcomeColor) ?? .secondary)
            }
            if !record.snapshot.isEmpty {
                Text("Compared with snapshot \(friendlyStamp(record.snapshot))")
                    .font(.caption)
                    .foregroundStyle(.secondary)
            }
            fileList("Same size and date, different content", record.silent, record.silentCount, .red)
            fileList("Edited since the snapshot", record.changed, record.changedCount, .orange)
            fileList("New in the source", record.missing, record.missingCount, .secondary)
            fileList("Deleted from the source", record.extra, record.extraCount, .secondary)
        }
    }

    private var logSection: some View {
        section("Log", icon: "text.alignleft") {
            LogView(pairID: draft.id, running: model.running?.job.pairID == draft.id)
        }
    }

    private var protectionSection: some View {
        section("What this protects against", icon: "lock.shield") {
            Text("Each snapshot is a separate dated folder, so files that ransomware encrypts or deletes in the source can be restored from an earlier snapshot. Hard links mean snapshots share the same disk blocks for unchanged files: this is not a second copy of the data. Read-only snapshots stop casual rewrites, but anything running as you can undo that. Keep the destination on a separate drive and disconnect it (or use a second, offline drive) for real isolation.")
                .foregroundStyle(.secondary)
                .fixedSize(horizontal: false, vertical: true)
        }
    }

    @ViewBuilder private var deleteSection: some View {
        if model.confirmDelete {
            HStack {
                Text("Remove this folder pair? Snapshots already on disk are kept.")
                Button("Remove") { model.deleteSelected() }
                    .foregroundStyle(.red)
                Button("Cancel") { model.confirmDelete = false }
            }
        } else {
            Button("Remove folder pair…", role: .destructive) { model.confirmDelete = true }
        }
    }

    private var saveBar: some View {
        VStack(alignment: .leading, spacing: 4) {
            Divider()
            HStack {
                Button("Save changes") { model.saveDraft() }
                    .buttonStyle(.borderedProminent)
                    .disabled(!dirty || !errors.isEmpty)
                if model.draftIsNew {
                    Button("Discard") { model.discardNew() }
                } else {
                    Button("Revert") { model.revertDraft() }
                        .disabled(!dirty)
                }
                if dirty {
                    Text("Unsaved changes").foregroundStyle(.orange)
                }
                Spacer()
            }
            ForEach(errors, id: \.self) { error in
                Text(error).foregroundStyle(.red).font(.callout)
            }
        }
        .padding(12)
        .background(.bar)
    }

    private func nextText(_ kind: JobKind) -> String {
        let expression = kind.schedule(of: draft)
        if expression.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty { return "Manual only" }
        if !draft.enabled { return "Paused" }
        if model.waiting.contains(draft.id) { return "Waiting for the destination drive" }
        guard let next = model.nextRuns[AnchorKey(pairID: draft.id, kind: kind)] else { return "—" }
        if next <= model.now { return "Due now" }
        return relativeFuture(next, now: model.now)
    }

    private func showSnapshots() {
        let destination = URL(fileURLWithPath: draft.destination, isDirectory: true)
        let latest = destination.appendingPathComponent(latestNameLink)
        if FileManager.default.fileExists(atPath: latest.path) {
            NSWorkspace.shared.activateFileViewerSelecting([latest])
        } else {
            NSWorkspace.shared.open(destination)
        }
    }

    private func bytes(_ value: UInt64?) -> String {
        value.map(formatBytes) ?? "—"
    }

    private func statusRow<Content: View>(_ title: String, @ViewBuilder content: () -> Content) -> some View {
        HStack(alignment: .firstTextBaseline, spacing: 12) {
            Text(title).frame(width: 110, alignment: .leading).foregroundStyle(.secondary)
            content()
            Spacer(minLength: 0)
        }
    }

    private func folderRow(_ title: String, path: String, placeholder: String, choose: @escaping () -> Void) -> some View {
        VStack(alignment: .leading, spacing: 6) {
            Text(title).font(.caption).foregroundStyle(.secondary)
            HStack(spacing: 8) {
                Image(systemName: title == "Source" ? "folder" : "externaldrive")
                    .foregroundStyle(.secondary)
                if path.isEmpty {
                    Text(placeholder).foregroundStyle(.tertiary)
                } else {
                    Text(shortPath(path))
                        .font(.system(.body, design: .monospaced))
                        .lineLimit(1)
                        .truncationMode(.middle)
                        .textSelection(.enabled)
                        .help(path)
                }
                Spacer(minLength: 8)
                Button("Choose…", action: choose)
                    .controlSize(.small)
            }
            .padding(.horizontal, 10)
            .padding(.vertical, 8)
            .background(Color.primary.opacity(0.04), in: RoundedRectangle(cornerRadius: 8, style: .continuous))
        }
    }

    private func scheduleRow(_ title: String, which: String, binding: Binding<String>) -> some View {
        HStack(alignment: .firstTextBaseline) {
            Text(title).frame(width: 110, alignment: .leading)
            ScheduleEditor(pairID: draft.id, which: which, expression: binding)
        }
    }

    private func retentionRow(_ title: String, value: Binding<Int>, range: ClosedRange<Int>, unit: String) -> some View {
        HStack {
            Text(title).frame(width: 140, alignment: .leading)
            NumberField(value: value, range: range)
            Text(unit).foregroundStyle(.secondary)
            Spacer()
        }
    }

    private func fileList(_ title: String, _ files: [String], _ count: Int, _ color: Color) -> some View {
        Group {
            if count > 0 {
                DisclosureGroup {
                    ForEach(files, id: \.self) { name in
                        Text(name).font(.system(.caption, design: .monospaced)).textSelection(.enabled)
                    }
                    if count > files.count {
                        Text("…and \(count - files.count) more (see log)")
                            .font(.caption)
                            .foregroundStyle(.secondary)
                    }
                } label: {
                    Text("\(title) (\(count))").foregroundStyle(color)
                }
            }
        }
    }

    private func section<Content: View>(_ title: String, icon: String, @ViewBuilder content: () -> Content) -> some View {
        VStack(alignment: .leading, spacing: 12) {
            Label(title, systemImage: icon)
                .font(.headline)
                .foregroundStyle(.primary)
            content()
        }
        .padding(16)
        .macCard()
    }
}

private struct MetricTile: View {
    var title: String
    var value: String
    var detail: String

    var body: some View {
        VStack(alignment: .leading, spacing: 4) {
            Text(title).font(.caption).foregroundStyle(.secondary)
            Text(value)
                .font(.title3.weight(.semibold))
                .lineLimit(1)
                .minimumScaleFactor(0.6)
            Text(detail)
                .font(.caption2)
                .foregroundStyle(.tertiary)
                .lineLimit(2)
                .fixedSize(horizontal: false, vertical: true)
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .padding(12)
        .background(Color.primary.opacity(0.045), in: RoundedRectangle(cornerRadius: 8, style: .continuous))
    }
}

private extension View {
    func macCard() -> some View {
        background {
            RoundedRectangle(cornerRadius: 12, style: .continuous)
                .fill(Color(nsColor: .controlBackgroundColor))
        }
        .overlay {
            RoundedRectangle(cornerRadius: 12, style: .continuous)
                .strokeBorder(Color.primary.opacity(0.07))
        }
    }
}

struct NumberField: View {
    @Binding var value: Int
    var range: ClosedRange<Int>

    var body: some View {
        HStack(spacing: 2) {
            TextField("", value: $value, format: IntegerFormatStyle<Int>().grouping(.never))
                .frame(width: 64)
                .multilineTextAlignment(.trailing)
            Stepper("", value: $value, in: range).labelsHidden()
        }
        .onChange(of: value) { newValue in
            let clamped = min(range.upperBound, max(range.lowerBound, newValue))
            if clamped != newValue { value = clamped }
        }
    }
}

struct ScheduleEditor: View {
    @EnvironmentObject private var model: AppModel
    var pairID: String
    var which: String
    @Binding var expression: String

    private var key: String { "\(pairID)|\(which)" }

    private var preset: Preset {
        model.customSchedule.contains(key) ? .custom : .fromCron(expression)
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 4) {
            HStack {
                Picker("Schedule", selection: Binding(get: { preset.kind }, set: applyKind)) {
                    ForEach(PresetKind.allCases) { kind in
                        Text(kind.label).tag(kind)
                    }
                }
                .labelsHidden()
                .frame(width: 160)
                controls
            }
            if preset == .custom {
                if let error = validateSchedule(expression) {
                    Text(error).foregroundStyle(.red).font(.caption)
                } else if let next = nextAfter(expression, after: model.now) {
                    Text("Next match: \(SelfDate.weekday.string(from: next))")
                        .font(.caption)
                        .foregroundStyle(.secondary)
                }
            }
        }
        .onAppear {
            if Preset.fromCron(expression) == .custom {
                model.customSchedule.insert(key)
            }
        }
    }

    @ViewBuilder private var controls: some View {
        switch preset {
        case .manual, .custom:
            if preset == .custom {
                TextField("min hour day month weekday", text: $expression)
                    .font(.system(.body, design: .monospaced))
                    .frame(width: 180)
                    .help("Standard cron: minute hour day-of-month month day-of-week (0 = Sunday). Example: 30 */4 * * 1-5")
            }
        case .hourly(let minute):
            Text("at minute")
            NumberField(value: minuteBinding(minute), range: 0 ... 59)
        case .daily(let hour, let minute):
            Text("at")
            timeFields(hour: hour, minute: minute)
        case .weekly(let weekday, let hour, let minute):
            Text("on")
            Picker("Weekday", selection: weekdayBinding(weekday, hour: hour, minute: minute)) {
                ForEach(0 ..< 7, id: \.self) { index in
                    Text(weekdays[index]).tag(index)
                }
            }
            .labelsHidden()
            .frame(width: 120)
            Text("at")
            timeFields(hour: hour, minute: minute)
        }
    }

    private func applyKind(_ kind: PresetKind) {
        if kind == preset.kind { return }
        if kind == .custom {
            model.customSchedule.insert(key)
            if expression.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty {
                expression = "0 2 * * *"
            }
            return
        }
        model.customSchedule.remove(key)
        if let cron = kind.defaultPreset().toCron() {
            expression = cron
        }
    }

    private func minuteBinding(_ minute: Int) -> Binding<Int> {
        Binding(get: { minute }, set: { expression = Preset.hourly(minute: $0).toCron() ?? expression })
    }

    private func timeFields(hour: Int, minute: Int) -> some View {
        HStack(spacing: 2) {
            NumberField(value: Binding(
                get: { hour },
                set: { newHour in
                    expression = replacement(hour: newHour, minute: minute)
                }
            ), range: 0 ... 23)
            Text(":")
            NumberField(value: Binding(
                get: { minute },
                set: { newMinute in
                    expression = replacement(hour: hour, minute: newMinute)
                }
            ), range: 0 ... 59)
        }
    }

    private func weekdayBinding(_ weekday: Int, hour: Int, minute: Int) -> Binding<Int> {
        Binding(
            get: { weekday },
            set: { expression = Preset.weekly(weekday: $0, hour: hour, minute: minute).toCron() ?? expression }
        )
    }

    private func replacement(hour: Int, minute: Int) -> String {
        switch preset {
        case .daily:
            return Preset.daily(hour: hour, minute: minute).toCron() ?? expression
        case .weekly(let weekday, _, _):
            return Preset.weekly(weekday: weekday, hour: hour, minute: minute).toCron() ?? expression
        default:
            return expression
        }
    }
}

struct ExcludesEditor: View {
    var pairID: String
    @Binding var excludes: [String]
    @State private var text = ""
    @State private var pair = ""

    var body: some View {
        TextEditor(text: $text)
            .font(.system(.body, design: .monospaced))
            .scrollContentBackground(.hidden)
            .padding(8)
            .frame(minHeight: 88, maxHeight: 140)
            .background(Color.primary.opacity(0.04), in: RoundedRectangle(cornerRadius: 8, style: .continuous))
            .onAppear { load() }
            .onChange(of: pairID) { _ in load() }
            .onChange(of: text) { newValue in
                let lines = textLines(newValue)
                if lines != excludes { excludes = lines }
            }
    }

    private func load() {
        guard pair != pairID else { return }
        pair = pairID
        text = excludes.joined(separator: "\n")
    }
}

struct RetentionPreview: View {
    var destination: String
    var policy: Retention
    var token: Int
    @State private var snapshots: [(String, SnapshotStamp)] = []
    @State private var latest: String?

    var body: some View {
        Group {
            if destination.isEmpty {
                EmptyView()
            } else if snapshots.isEmpty {
                Text("No snapshots yet.").foregroundStyle(.secondary)
            } else {
                let decisions = retentionPlan(snapshots: snapshots, policy: policy, latest: latest)
                let kept = decisions.filter(\.keep).count
                let removed = decisions.count - kept
                Text("\(decisions.count) snapshots on disk: this policy keeps \(kept) and removes \(removed) after the next backup.")
                    .foregroundStyle(removed > 0 ? Color.orange : Color.green)
                DisclosureGroup("Preview") {
                    ForEach(decisions, id: \.name) { decision in
                        HStack {
                            Text(friendlyStamp(decision.name))
                                .font(.system(.body, design: .monospaced))
                            Spacer()
                            if decision.keep {
                                Text("keep · \(decision.keptBy.map(\.label).joined(separator: ", "))")
                                    .foregroundStyle(.green)
                            } else {
                                Text("remove").foregroundStyle(.red)
                            }
                        }
                    }
                }
            }
        }
        .onAppear { reload() }
        .onChange(of: destination) { _ in reload() }
        .onChange(of: token) { _ in reload() }
        .onReceive(Timer.publish(every: 5, on: .main, in: .common).autoconnect()) { _ in
            reload()
        }
    }

    private func reload() {
        guard !destination.isEmpty else {
            snapshots = []
            latest = nil
            return
        }
        let url = URL(fileURLWithPath: destination, isDirectory: true)
        snapshots = (try? listSnapshots(dest: url)) ?? []
        latest = latestSnapshotName(dest: url)
    }
}

struct LogView: View {
    var pairID: String
    var running: Bool
    @State private var text = ""

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            Button("Open log file") {
                NSWorkspace.shared.open(Paths.logFile(pairID: pairID))
            }
            .controlSize(.small)
            ScrollView {
                Text(text.isEmpty ? "No activity yet." : text)
                    .font(.system(.caption, design: .monospaced))
                    .foregroundStyle(text.isEmpty ? .secondary : .primary)
                    .textSelection(.enabled)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .padding(10)
            }
            .frame(height: 200)
            .background(Color.primary.opacity(0.04), in: RoundedRectangle(cornerRadius: 8, style: .continuous))
        }
        .onAppear { text = readLogTail(pairID: pairID) }
        .onChange(of: pairID) { _ in text = readLogTail(pairID: pairID) }
        .onReceive(Timer.publish(every: running ? 2 : 5, on: .main, in: .common).autoconnect()) { _ in
            text = readLogTail(pairID: pairID)
        }
    }
}

private func directoryExists(_ path: String) -> Bool {
    var isDir: ObjCBool = false
    return FileManager.default.fileExists(atPath: path, isDirectory: &isDir) && isDir.boolValue
}

private func readLogTail(pairID: String) -> String {
    let url = Paths.logFile(pairID: pairID)
    guard let handle = try? FileHandle(forReadingFrom: url) else { return "" }
    defer { try? handle.close() }
    let length = handle.seekToEndOfFile()
    let maxBytes: UInt64 = 48 * 1024
    let start = length > maxBytes ? length - maxBytes : 0
    handle.seek(toFileOffset: start)
    let data = handle.readDataToEndOfFile()
    let text = String(data: data, encoding: .utf8) ?? ""
    var lines = textLines(text)
    if start > 0, !lines.isEmpty { lines.removeFirst() }
    return lines.suffix(300).joined(separator: "\n")
}
