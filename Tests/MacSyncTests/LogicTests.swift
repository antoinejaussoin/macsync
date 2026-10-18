import Foundation
import XCTest
@testable import MacSync

final class MacSyncLogicTests: XCTestCase {
    func testFormatBytes() {
        XCTAssertEqual(formatBytes(512), "512 B")
        XCTAssertEqual(formatBytes(3_400_000), "3.4 MB")
        XCTAssertEqual(formatBytes(1_500_000_000_000), "1.5 TB")
    }

    func testRelativeTime() {
        let now = localDate(2026, 10, 11, 8, 0)
        XCTAssertEqual(relativeTime(now.addingTimeInterval(-30), now: now), "just now")
        XCTAssertEqual(relativeTime(now.addingTimeInterval(-5 * 60), now: now), "5 min ago")
        XCTAssertEqual(relativeTime(now.addingTimeInterval(-3 * 3600), now: now), "3 h ago")
        XCTAssertEqual(relativeTime(now.addingTimeInterval(-3 * 86_400), now: now), "3 days ago")
        XCTAssertEqual(relativeFuture(now.addingTimeInterval(30 * 60), now: now), "in 30 min (08:30)")
    }

    func testVersionGate() {
        XCTAssertEqual(try parseRsyncVersion("rsync  version 3.4.1  protocol version 32\nCopyright").get(), "3.4.1")
        XCTAssertTrue(parseRsyncVersion("openrsync: protocol version 29\nrsync version 2.6.9 compatible").errorMessage.contains("not GNU"))
        XCTAssertTrue(parseRsyncVersion("rsync  version 2.6.9  protocol version 29").errorMessage.contains("too old"))
        XCTAssertTrue(parseRsyncVersion("rsync  version 3.0.9  protocol version 30").errorMessage.contains("too old"))
        XCTAssertTrue(parseRsyncVersion("").errorMessage.contains("not GNU"))
    }

    func testBundledRsyncIsPreferred() {
        XCTAssertEqual(
            rsyncSearchPaths(overridePath: "/custom/rsync", bundledPath: "/app/rsync"),
            ["/custom/rsync"]
        )
        XCTAssertEqual(
            rsyncSearchPaths(overridePath: nil, bundledPath: "/app/rsync"),
            ["/app/rsync", "/opt/homebrew/bin/rsync", "/usr/local/bin/rsync"]
        )
        XCTAssertEqual(
            rsyncSearchPaths(overridePath: nil, bundledPath: nil),
            ["/opt/homebrew/bin/rsync", "/usr/local/bin/rsync"]
        )
    }

    func testSystemRsyncIsRejected() {
        let status = locateRsync(overridePath: "/usr/bin/rsync")
        XCTAssertTrue(status.message?.contains("openrsync") == true, status.message ?? "")
    }

    func testBackupArguments() {
        let excludes = [".DS_Store", "  ", "-weird name"]
        let args = backupArguments(BackupArguments(
            source: "/Users/a/My Docs",
            target: "/Volumes/B/docs/.inprogress-2026-10-11T081300",
            linkDest: nil,
            excludes: excludes,
            readOnly: false
        ))
        XCTAssertEqual(args, [
            "-aHAX", "--delete", "--delete-excluded", "--info=stats2,progress2",
            "--exclude=.DS_Store", "--exclude=-weird name",
            "/Users/a/My Docs/", "/Volumes/B/docs/.inprogress-2026-10-11T081300/",
        ])
    }

    func testBackupArgumentsLinkDest() {
        let args = backupArguments(BackupArguments(
            source: "/src/",
            target: "/dst/.inprogress-x",
            linkDest: "/dst/2026-10-10T020000",
            excludes: [],
            readOnly: true
        ))
        XCTAssertTrue(args.contains("--chmod=Fa-w"))
        XCTAssertTrue(args.contains("--link-dest=/dst/2026-10-10T020000"))
        XCTAssertEqual(args[args.count - 2], "/src/")
    }

    func testIntegrityArguments() {
        let args = integrityArguments(source: "/src", snapshot: "/dst/2026", excludes: [], readOnly: true)
        XCTAssertTrue(args.contains("--dry-run"))
        XCTAssertTrue(args.contains("--checksum"))
        XCTAssertTrue(args.contains("--chmod=Fa-w"))
        XCTAssertFalse(args.contains { $0.hasPrefix("--link-dest") })
    }

    func testProgressLines() {
        let progress = parseProgress("      1,234,567  45%   12.34MB/s    0:00:10 (xfr#5, to-chk=10/100)")
        XCTAssertEqual(progress?.bytes, 1_234_567)
        XCTAssertEqual(progress?.percent, 45)
        XCTAssertEqual(progress?.rate, "12.34MB/s")
        XCTAssertEqual(progress?.filesTransferred, 5)
        XCTAssertEqual(progress?.scanning, false)
        XCTAssertEqual(progress?.filesTotal, 100)
        XCTAssertEqual(progress?.filesChecked, 90)

        let scanning = parseProgress("              0   0%    0.00kB/s    0:00:00 (xfr#0, ir-chk=1000/1031)")
        XCTAssertEqual(scanning?.percent, 0)
        XCTAssertEqual(scanning?.scanning, true)
        XCTAssertEqual(scanning?.filesTotal, 1031)
        XCTAssertNil(scanning?.filesChecked)
        XCTAssertEqual(etaSeconds("0:02:10"), 130)
        XCTAssertEqual(aboutRemaining("0:02:10"), "about 2 min left")
        XCTAssertEqual(shortRemaining("0:00:45"), "45s")
        XCTAssertNil(aboutRemaining("0:00:00"))
        XCTAssertEqual(guessedProgress(elapsed: 60, eta: "0:02:00") ?? -1, 60.0 / 180.0, accuracy: 0.001)
        XCTAssertNil(guessedProgress(elapsed: 10, eta: "0:00:00"))
        XCTAssertNil(parseProgress("sending incremental file list"))
        XCTAssertNil(parseProgress("Number of files: 1,031 (reg: 1,000, dir: 31)"))
    }

    func testStatsSummary() {
        let lines = [
            "Number of files: 1,031 (reg: 1,000, dir: 31)",
            "Number of regular files transferred: 12",
            "Total file size: 9,999,999 bytes",
            "Total transferred file size: 3,400,000 bytes",
        ]
        XCTAssertEqual(summarizeStats(lines), "12 files copied (3.4 MB) of 1031")
    }

    func testStamps() {
        XCTAssertNotNil(parseStamp("2026-10-11T081300"))
        XCTAssertNil(parseStamp("2026-10-11T08130"))
        XCTAssertNil(parseStamp(".inprogress-2026-10-11T081300"))
        XCTAssertNil(parseStamp("latest"))
        XCTAssertNil(parseStamp("2026-13-11T081300"))
    }

    func testDefaultRetention() {
        let snaps = dailyHistory(from: (2024, 3, 1), to: (2026, 10, 11))
        let decisions = retentionPlan(snapshots: snaps, policy: .default, latest: "2026-10-11T020000")
        var expected = (12 ... 30).reversed().map { String(format: "2026-09-%02dT020000", $0) }
        expected.insert(contentsOf: (1 ... 11).reversed().map { String(format: "2026-10-%02dT020000", $0) }, at: 0)
        expected.append("2026-08-31T020000")
        expected.append("2025-12-31T020000")
        expected.append("2024-12-31T020000")
        XCTAssertEqual(decisions.filter(\.keep).map(\.name), expected)
        XCTAssertEqual(decisions[0].keptBy, [.latest, .last, .monthly, .yearly])
        XCTAssertEqual(decisions.first { $0.name == "2026-08-31T020000" }?.keptBy, [.monthly])
    }

    func testMonthCoveredByLastIsNotDuplicated() {
        let snaps = dailyHistory(from: (2026, 10, 1), to: (2026, 11, 9))
        let policy = Retention(keepLast: 30, keepDaily: 0, keepWeekly: 0, keepMonthly: 2, keepYearly: .count(0))
        let kept = retentionPlan(snapshots: snaps, policy: policy, latest: nil).filter(\.keep).map(\.name)
        XCTAssertEqual(kept.count, 30)
        XCTAssertTrue(kept.contains("2026-10-31T020000"))
    }

    func testYearlyForeverVersusCap() {
        let snaps = dailyHistory(from: (2020, 1, 1), to: (2026, 10, 11))
        func onlyYearly(_ yearly: KeepYearly) -> Retention {
            Retention(keepLast: 1, keepDaily: 0, keepWeekly: 0, keepMonthly: 0, keepYearly: yearly)
        }
        let forever = retentionPlan(snapshots: snaps, policy: onlyYearly(.forever), latest: nil).filter(\.keep).map(\.name)
        XCTAssertEqual(forever.count, 7, forever.joined(separator: ", "))
        XCTAssertTrue(forever.contains("2020-12-31T020000"))
        let capped = retentionPlan(snapshots: snaps, policy: onlyYearly(.count(3)), latest: nil).filter(\.keep).map(\.name)
        XCTAssertEqual(capped, ["2026-10-11T020000", "2025-12-31T020000", "2024-12-31T020000"])
    }

    func testZeroTiersKeepLatest() {
        let snaps = dailyHistory(from: (2026, 1, 1), to: (2026, 3, 1))
        let none = Retention(keepLast: 0, keepDaily: 0, keepWeekly: 0, keepMonthly: 0, keepYearly: .count(0))
        let decisions = retentionPlan(snapshots: snaps, policy: none, latest: "2026-02-01T020000")
        XCTAssertEqual(decisions.filter(\.keep).map(\.name), ["2026-02-01T020000"])
        XCTAssertEqual(decisions.first { $0.keep }?.keptBy, [.latest])
    }

    func testDailyAndWeeklyPickNewest() {
        var snaps: [(String, SnapshotStamp)] = []
        for day in 1 ... 3 {
            for hour in 0 ..< 24 {
                let name = String(format: "2026-06-%02dT%02d0000", day, hour)
                snaps.append((name, parseStamp(name)!))
            }
        }
        let policy = Retention(keepLast: 2, keepDaily: 3, keepWeekly: 1, keepMonthly: 0, keepYearly: .count(0))
        let kept = retentionPlan(snapshots: snaps, policy: policy, latest: nil).filter(\.keep).map(\.name)
        XCTAssertEqual(kept, ["2026-06-03T230000", "2026-06-03T220000", "2026-06-02T230000", "2026-06-01T230000"])
    }

    func testSchedules() {
        let next = nextAfter("30 2 * * *", after: localDate(2026, 10, 11, 8, 0))
        XCTAssertEqual(next, localDate(2026, 10, 12, 2, 30))
        let sunday = nextAfter("0 9 * * 0", after: localDate(2026, 10, 10, 12, 0))
        XCTAssertEqual(sunday, localDate(2026, 10, 11, 9, 0))
        XCTAssertNil(nextAfter("", after: localDate(2026, 10, 11, 8, 0)))
        XCTAssertNotNil(validateSchedule("0 0 0 * * *"))
        XCTAssertNotNil(validateSchedule("nope"))
    }

    func testCatchUpAfterSleep() {
        let lastRun = localDate(2026, 10, 8, 2, 0)
        let woke = localDate(2026, 10, 11, 9, 0)
        XCTAssertTrue(scheduleIsDue("0 2 * * *", anchor: lastRun, now: woke))
        let caughtUp = localDate(2026, 10, 11, 9, 0)
        XCTAssertFalse(scheduleIsDue("0 2 * * *", anchor: caughtUp, now: localDate(2026, 10, 11, 23, 59)))
        XCTAssertTrue(scheduleIsDue("0 2 * * *", anchor: caughtUp, now: localDate(2026, 10, 12, 2, 0)))
    }

    func testPresetsRoundTrip() {
        let presets: [Preset] = [
            .manual,
            .hourly(minute: 15),
            .daily(hour: 2, minute: 0),
            .weekly(weekday: 1, hour: 3, minute: 30),
        ]
        for preset in presets {
            let cron = preset.toCron()!
            XCTAssertEqual(Preset.fromCron(cron), preset, cron)
            XCTAssertNil(validateSchedule(cron))
        }
        XCTAssertEqual(Preset.fromCron("*/15 9-17 * * 1-5"), .custom)
    }

    func testClassifiesItemizedOutput() {
        let lines = [
            ">fc........|photos/bitrot.jpg",
            ">fcst......|notes/edited.md",
            ">f.st......|notes/grown.md",
            ">f+++++++++|new.txt",
            "cd+++++++++|newdir/",
            "*deleting  |gone.txt",
            "*deleting  |gonedir/",
            ".f..t......|touched.txt",
            ".d..t......|notes/",
            "cL+++++++++|link",
            "not itemized output",
        ]
        let differences = classifyItemized(lines)
        XCTAssertEqual(differences.silent, ["photos/bitrot.jpg"])
        XCTAssertEqual(differences.changed, ["notes/edited.md", "notes/grown.md"])
        XCTAssertEqual(differences.missing, ["new.txt", "link"])
        XCTAssertEqual(differences.extra, ["gone.txt"])
        XCTAssertEqual(differences.metadataOnly, 1)
    }

    func testIntegrityOutcomes() {
        let clean = integrityRecord(from: classifyItemized([]), snapshot: "s")
        XCTAssertEqual(clean.outcome, .success)
        let drift = integrityRecord(from: classifyItemized([">fcst......|a"]), snapshot: "s")
        XCTAssertEqual(drift.outcome, .warning)
        let silent = integrityRecord(from: classifyItemized([">fc........|a", ">f+++++++++|b"]), snapshot: "s")
        XCTAssertEqual(silent.outcome, .failed)
        XCTAssertEqual(silent.silentCount, 1)
        XCTAssertEqual(silent.missingCount, 1)
    }

    func testUnsafePaths() {
        func unsafe(_ source: String, _ destination: String) -> Bool {
            if case .failure(.unsafe) = checkPairPaths(source: source, destination: destination) { return true }
            return false
        }
        XCTAssertTrue(unsafe("/Users/a/Docs", "/"))
        XCTAssertTrue(unsafe("/Users/a/Docs", "/Users/a/Docs"))
        XCTAssertTrue(unsafe("/Users/a/Docs", "/Users/a/Docs/backup"))
        XCTAssertTrue(unsafe("/Volumes/B/x/Docs", "/Volumes/B/x"))
        XCTAssertTrue(unsafe("relative", "/Volumes/B"))
        if case .failure(.notSet) = checkPairPaths(source: "", destination: "/Volumes/B") {
        } else {
            XCTFail("empty source should be not set")
        }
    }

    func testSimilarPrefixIsNotContainment() throws {
        let root = temporaryDirectory()
        let source = root.appendingPathComponent("Docs")
        let destination = root.appendingPathComponent("Docs-backup")
        try FileManager.default.createDirectory(at: source, withIntermediateDirectories: true)
        try FileManager.default.createDirectory(at: destination, withIntermediateDirectories: true)
        XCTAssertNoThrow(try checkPairPaths(source: source.path, destination: destination.path).get())
    }

    func testMissingDestination() throws {
        let root = temporaryDirectory()
        if case .failure(.destinationMissing) = checkPairPaths(source: root.path, destination: "/Volumes/__macsync_missing__") {
        } else {
            XCTFail("expected destination missing")
        }
    }

    func testConfigRoundTrip() throws {
        var config = Config.empty
        var pair = Pair.makeNew()
        pair.name = "Documents"
        pair.source = "/Users/a/Documents"
        pair.destination = "/Volumes/Backup/Documents"
        pair.retention.keepYearly = .count(5)
        config.pairs = [pair, Pair.makeNew()]
        let text = config.toml()
        XCTAssertTrue(text.contains("[[pair]]"))
        XCTAssertTrue(text.contains("keep_yearly = 5"))
        XCTAssertTrue(text.contains("keep_yearly = \"forever\""))
        let parsed = try Config(parsing: TOMLTable.parse(text))
        XCTAssertEqual(parsed.pairs[0].name, "Documents")
        XCTAssertEqual(parsed.pairs[0].retention.keepYearly, .count(5))
        XCTAssertEqual(parsed.pairs[1].retention.keepYearly, .forever)
        XCTAssertEqual(parsed.pairs[1].excludes, [".DS_Store"])
        XCTAssertTrue(parsed.pairs[1].readOnly)
    }

    func testRustConfigSample() throws {
        let text = """
        [[pair]]
        id = "abc"
        name = "Documents"
        source = "/Users/a/Documents"
        destination = "/Volumes/Backup/Documents snapshots"
        enabled = true
        backup_schedule = "0 * * * *"
        integrity_schedule = "0 4 * * 0"
        excludes = [
            ".DS_Store",
            "node_modules/",
        ]
        read_only = true

        [pair.retention]
        keep_last = 30
        keep_daily = 0
        keep_weekly = 0
        keep_monthly = 3
        keep_yearly = 5

        [[pair]]
        id = "def"
        name = 'A "quoted" name'
        source = ""
        destination = ""
        enabled = true
        backup_schedule = "0 2 * * *"
        integrity_schedule = ""
        excludes = [".DS_Store"]
        read_only = true

        [pair.retention]
        keep_last = 30
        keep_daily = 0
        keep_weekly = 0
        keep_monthly = 3
        keep_yearly = "forever"
        """
        let config = try Config(parsing: TOMLTable.parse(text))
        XCTAssertEqual(config.pairs.count, 2)
        XCTAssertEqual(config.pairs[0].excludes, [".DS_Store", "node_modules/"])
        XCTAssertEqual(config.pairs[0].retention.keepYearly, .count(5))
        XCTAssertEqual(config.pairs[1].name, "A \"quoted\" name")
        XCTAssertEqual(config.pairs[1].retention.keepYearly, .forever)
    }

    func testMissingConfigFieldsTakeDefaults() throws {
        let text = "[[pair]]\nid = \"x\"\nname = \"Docs\"\n[pair.retention]\nkeep_last = 10\n"
        let config = try Config(parsing: TOMLTable.parse(text))
        let pair = config.pairs[0]
        XCTAssertEqual(pair.retention.keepLast, 10)
        XCTAssertEqual(pair.retention.keepMonthly, 3)
        XCTAssertEqual(pair.retention.keepYearly, .forever)
        XCTAssertTrue(pair.readOnly)
        XCTAssertEqual(pair.excludes, [".DS_Store"])
    }

    func testStateRoundTrip() throws {
        let text = """
        [pairs.aaa]
        last_success = "2026-10-11T08:00:00+01:00"

        [pairs.hist]
        last_success = "2026-10-11T08:00:00+01:00"

        [pairs.hist.last_backup]
        started = "2026-10-11T08:00:00+01:00"
        finished = "2026-10-11T08:00:00+01:00"
        outcome = "failed"
        message = "rsync exit 23: some files could not be copied"
        pruned = 0

        [pairs.hist.last_integrity]
        started = "2026-10-11T08:00:00+01:00"
        finished = "2026-10-11T08:00:00+01:00"
        outcome = "failed"
        message = "1 file(s) differ"
        snapshot = "2026-10-11T020000"
        silent = ["2024/IMG_0042.HEIC"]
        silent_count = 1
        changed = ["notes.md"]
        changed_count = 1
        missing = []
        missing_count = 0
        extra = []
        extra_count = 0

        [pairs.hist.stats]
        measured = "2026-10-11T08:00:00+01:00"
        source_bytes = 52000000000
        destination_bytes = 61500000000
        free_bytes = 900000000000
        filesystem = "apfs"
        snapshot_count = 8
        oldest = "2025-05-01T020000"
        newest = "2026-10-11T020000"
        """
        let state = try SyncState(parsing: TOMLTable.parse(text))
        XCTAssertEqual(state.pairs["hist"]?.lastBackup?.outcome, .failed)
        XCTAssertEqual(state.pairs["hist"]?.stats?.sourceBytes, 52_000_000_000)
        XCTAssertEqual(state.pairs["hist"]?.lastIntegrity?.silent, ["2024/IMG_0042.HEIC"])
        XCTAssertNotNil(state.pairs["aaa"]?.lastSuccess)
        let encoded = state.toml()
        let again = try SyncState(parsing: TOMLTable.parse(encoded))
        XCTAssertEqual(again.pairs["hist"]?.lastBackup?.message, state.pairs["hist"]?.lastBackup?.message)
        XCTAssertEqual(again.pairs["hist"]?.stats?.freeBytes, 900_000_000_000)
    }

    func testQueuesOnceAndWaitsForDestination() {
        let root = temporaryDirectory()
        var pair = Pair.makeNew()
        pair.id = "p"
        pair.source = root.appendingPathComponent("src").path
        pair.destination = root.path
        pair.backupSchedule = "0 2 * * *"
        pair.integritySchedule = "0 3 * * 0"
        var data = SchedulerData(config: Config(rsyncPath: nil, notify: .default, pairs: [pair]), state: .empty, rsync: .missing("not needed"))
        XCTAssertFalse(data.tick(now: localDate(2026, 10, 11, 8, 0)))
        XCTAssertTrue(data.queue.isEmpty)
        XCTAssertEqual(data.nextRun(pair: data.config.pairs[0], kind: .backup, now: localDate(2026, 10, 11, 8, 0)), localDate(2026, 10, 12, 2, 0))
        XCTAssertTrue(data.tick(now: localDate(2026, 10, 12, 2, 0)))
        XCTAssertEqual(data.queue.count, 1)
        XCTAssertFalse(data.tick(now: localDate(2026, 10, 12, 2, 0)))
        data.queue.removeAll()
        data.config.pairs[0].destination = root.appendingPathComponent("unplugged").path
        XCTAssertTrue(data.tick(now: localDate(2026, 10, 12, 2, 1)))
        XCTAssertTrue(data.queue.isEmpty)
        XCTAssertTrue(data.waitingForDestination.contains("p"))
        data.config.pairs[0].destination = root.path
        XCTAssertTrue(data.tick(now: localDate(2026, 10, 12, 9, 0)))
        XCTAssertEqual(data.queue.first?.kind, .backup)
        XCTAssertTrue(data.waitingForDestination.isEmpty)
    }

    func testLastAttemptDrivesCatchUp() {
        let root = temporaryDirectory()
        var pair = Pair.makeNew()
        pair.id = "p"
        pair.destination = root.path
        var data = SchedulerData(config: Config(rsyncPath: nil, notify: .default, pairs: [pair]), state: .empty, rsync: .missing("not needed"))
        data.state.pairs["p"] = PairState(
            lastBackup: RunRecord(
                started: localDate(2026, 10, 8, 2, 0),
                finished: localDate(2026, 10, 8, 2, 5),
                outcome: .success,
                message: "",
                snapshot: nil,
                pruned: 0
            ),
            lastSuccess: nil,
            lastIntegrity: nil,
            stats: nil
        )
        XCTAssertTrue(data.tick(now: localDate(2026, 10, 11, 9, 0)))
        XCTAssertEqual(data.queue.count, 1)
    }

    func testDisabledAndManualNeverQueue() {
        let root = temporaryDirectory()
        var manual = Pair.makeNew()
        manual.id = "m"
        manual.destination = root.path
        manual.backupSchedule = ""
        manual.integritySchedule = ""
        var disabled = Pair.makeNew()
        disabled.id = "d"
        disabled.destination = root.path
        disabled.enabled = false
        var data = SchedulerData(config: Config(rsyncPath: nil, notify: .default, pairs: [manual, disabled]), state: .empty, rsync: .missing("not needed"))
        _ = data.tick(now: localDate(2026, 10, 1, 0, 0))
        XCTAssertFalse(data.tick(now: localDate(2026, 10, 20, 0, 0)))
        XCTAssertTrue(data.queue.isEmpty)
    }

    func testPlist() throws {
        let xml = plistXML(executable: "/Applications/Mac & Sync.app/Contents/MacOS/macsync")
        XCTAssertTrue(xml.contains("<string>/Applications/Mac &amp; Sync.app/Contents/MacOS/macsync</string>"))
        XCTAssertTrue(xml.contains("<string>--background</string>"))
        XCTAssertTrue(xml.contains("<key>RunAtLoad</key>\n    <true/>"))
        XCTAssertFalse(xml.contains("KeepAlive"))
        let url = temporaryDirectory().appendingPathComponent("agent.plist")
        try plistXML(executable: "/Applications/MacSync.app/Contents/MacOS/MacSync").write(to: url, atomically: true, encoding: .utf8)
        let process = Process()
        process.executableURL = URL(fileURLWithPath: "/usr/bin/plutil")
        process.arguments = ["-lint", url.path]
        let pipe = Pipe()
        process.standardOutput = pipe
        try process.run()
        process.waitUntilExit()
        let output = String(data: pipe.fileHandleForReading.readDataToEndOfFile(), encoding: .utf8) ?? ""
        XCTAssertEqual(process.terminationStatus, 0, output)
    }

    func testSnapshotsAndPrune() throws {
        let root = temporaryDirectory()
        try touchSnapshot(root, "2026-10-10T020000")
        try touchSnapshot(root, "2026-10-11T020000")
        try touchSnapshot(root, ".inprogress-2026-10-12T020000")
        try Data("a file, not a snapshot".utf8).write(to: root.appendingPathComponent("2026-10-09T020000"))
        try setLatest(dest: root, name: "2026-10-11T020000")
        let names = try listSnapshots(dest: root).map(\.0)
        XCTAssertEqual(names, ["2026-10-11T020000", "2026-10-10T020000"])
        XCTAssertEqual(latestSnapshotName(dest: root), "2026-10-11T020000")
        XCTAssertEqual(try FileManager.default.destinationOfSymbolicLink(atPath: root.appendingPathComponent("latest").path), "2026-10-11T020000")

        let pruneRoot = temporaryDirectory()
        for day in 1 ... 5 {
            let name = String(format: "2026-10-0%dT020000", day)
            try touchSnapshot(pruneRoot, name)
            try makeReadOnly(pruneRoot.appendingPathComponent(name))
        }
        try setLatest(dest: pruneRoot, name: "2026-10-05T020000")
        let policy = Retention(keepLast: 2, keepDaily: 0, keepWeekly: 0, keepMonthly: 0, keepYearly: .count(0))
        setenv("MACSYNC_HOME", root.path, 1)
        defer { unsetenv("MACSYNC_HOME") }
        let log = try LogFile(pairID: "prune-test")
        let removed = try pruneSnapshots(dest: pruneRoot, policy: policy, log: log)
        XCTAssertEqual(removed, ["2026-10-01T020000", "2026-10-02T020000", "2026-10-03T020000"])
        XCTAssertEqual(try listSnapshots(dest: pruneRoot).map(\.0), ["2026-10-05T020000", "2026-10-04T020000"])
    }

    func testReadOnlyBlocksNewFiles() throws {
        let root = temporaryDirectory()
        try touchSnapshot(root, "2026-10-01T020000")
        let snap = root.appendingPathComponent("2026-10-01T020000")
        try makeReadOnly(snap)
        XCTAssertThrowsError(try Data("x".utf8).write(to: snap.appendingPathComponent("sub/new.txt")))
        try removeTree(snap)
        XCTAssertFalse(FileManager.default.fileExists(atPath: snap.path))
    }

    func testDestinationSuggestion() throws {
        let root = temporaryDirectory()
        let drive = root.appendingPathComponent("Drive")
        try FileManager.default.createDirectory(at: drive.appendingPathComponent("Other stuff"), withIntermediateDirectories: true)
        XCTAssertEqual(suggestedDestination(picked: drive.path, source: "/Users/a/Documents"), drive.appendingPathComponent("Documents snapshots").path)
        let empty = root.appendingPathComponent("Empty")
        try FileManager.default.createDirectory(at: empty, withIntermediateDirectories: true)
        XCTAssertEqual(suggestedDestination(picked: empty.path, source: "/Users/a/Documents"), empty.path)
    }

    func testEndToEndSnapshotsShareUnchangedFiles() throws {
        guard case .found(let info) = locateRsync(overridePath: nil) else {
            throw XCTSkip("rsync 3.1 or newer was not found")
        }
        let root = temporaryDirectory()
        setenv("MACSYNC_HOME", root.path, 1)
        defer { unsetenv("MACSYNC_HOME") }
        let source = root.appendingPathComponent("My Source")
        let destination = root.appendingPathComponent("Backup")
        try FileManager.default.createDirectory(at: source.appendingPathComponent("nested"), withIntermediateDirectories: true)
        try FileManager.default.createDirectory(at: destination, withIntermediateDirectories: true)
        try Data("unchanged".utf8).write(to: source.appendingPathComponent("same.txt"))
        try Data("v1".utf8).write(to: source.appendingPathComponent("nested/edit.txt"))
        var pair = Pair.makeNew()
        pair.source = source.path
        pair.destination = destination.path
        let log = try LogFile(pairID: "e2e")
        let cancel = CancelHandle()

        let first = try runBackup(pair: pair, rsync: info, log: log, cancel: cancel, onProgress: { _ in }).get()
        try Data("v2 longer".utf8).write(to: source.appendingPathComponent("nested/edit.txt"))
        Thread.sleep(forTimeInterval: 1.1)
        let second = try runBackup(pair: pair, rsync: info, log: log, cancel: cancel, onProgress: { _ in }).get()

        let a = destination.appendingPathComponent(first.snapshot)
        let b = destination.appendingPathComponent(second.snapshot)
        XCTAssertEqual(latestSnapshotName(dest: destination), second.snapshot)
        let sameA = try FileManager.default.attributesOfItem(atPath: a.appendingPathComponent("same.txt").path)[.systemFileNumber] as? NSNumber
        let sameB = try FileManager.default.attributesOfItem(atPath: b.appendingPathComponent("same.txt").path)[.systemFileNumber] as? NSNumber
        XCTAssertEqual(sameA, sameB)
        let editA = try FileManager.default.attributesOfItem(atPath: a.appendingPathComponent("nested/edit.txt").path)[.systemFileNumber] as? NSNumber
        let editB = try FileManager.default.attributesOfItem(atPath: b.appendingPathComponent("nested/edit.txt").path)[.systemFileNumber] as? NSNumber
        XCTAssertNotEqual(editA, editB)
        XCTAssertEqual(try String(contentsOf: a.appendingPathComponent("nested/edit.txt"), encoding: .utf8), "v1")
        XCTAssertThrowsError(try Data("x".utf8).write(to: b.appendingPathComponent("new.txt")))

        let clean = try runIntegrity(pair: pair, rsync: info, log: log, cancel: cancel).get()
        XCTAssertEqual(clean.outcome, .success, clean.message)

        let tampered = b.appendingPathComponent("nested/edit.txt")
        let modified = try FileManager.default.attributesOfItem(atPath: tampered.path)[.modificationDate] as? Date
        try FileManager.default.setAttributes([.posixPermissions: 0o644], ofItemAtPath: tampered.path)
        try Data("v2 LONGER".utf8).write(to: tampered)
        if let modified {
            try FileManager.default.setAttributes([.modificationDate: modified], ofItemAtPath: tampered.path)
        }
        try FileManager.default.setAttributes([.posixPermissions: 0o444], ofItemAtPath: tampered.path)
        let report = try runIntegrity(pair: pair, rsync: info, log: log, cancel: cancel).get()
        XCTAssertEqual(report.outcome, .failed, report.message)
        XCTAssertEqual(report.silent, ["nested/edit.txt"])
    }

    func testHardLinksCountOnce() throws {
        let root = temporaryDirectory()
        let a = root.appendingPathComponent("a")
        let b = root.appendingPathComponent("b")
        try FileManager.default.createDirectory(at: a, withIntermediateDirectories: true)
        try FileManager.default.createDirectory(at: b, withIntermediateDirectories: true)
        let big = a.appendingPathComponent("big")
        try Data(repeating: 1, count: 4_000_000).write(to: big)
        try FileManager.default.linkItem(at: big, to: b.appendingPathComponent("big"))
        let one = try duTotal([a.path]).get()
        let both = try duTotal([a.path, b.path]).get()
        XCTAssertGreaterThanOrEqual(one, 4_000_000)
        XCTAssertLessThan(both, one + 100_000, "linked file counted twice: \(one) vs \(both)")
    }
}

private extension Result where Success == String, Failure == AppError {
    var messageOrValue: String {
        switch self {
        case .success(let value): return value
        case .failure(let error): return error.message
        }
    }

    var errorMessage: String {
        switch self {
        case .success: return ""
        case .failure(let error): return error.message
        }
    }
}

private func localDate(_ year: Int, _ month: Int, _ day: Int, _ hour: Int, _ minute: Int) -> Date {
    var calendar = Calendar(identifier: .gregorian)
    calendar.timeZone = .current
    return calendar.date(from: DateComponents(year: year, month: month, day: day, hour: hour, minute: minute))!
}

private func dailyHistory(from: (Int, Int, Int), to: (Int, Int, Int)) -> [(String, SnapshotStamp)] {
    var calendar = Calendar(identifier: .gregorian)
    calendar.timeZone = .current
    var day = calendar.date(from: DateComponents(year: from.0, month: from.1, day: from.2))!
    let end = calendar.date(from: DateComponents(year: to.0, month: to.1, day: to.2))!
    var out: [(String, SnapshotStamp)] = []
    while day <= end {
        let parts = calendar.dateComponents([.year, .month, .day], from: day)
        let name = String(format: "%04d-%02d-%02dT020000", parts.year!, parts.month!, parts.day!)
        out.append((name, parseStamp(name)!))
        day = calendar.date(byAdding: .day, value: 1, to: day)!
    }
    return out
}

private func temporaryDirectory() -> URL {
    let url = URL(fileURLWithPath: NSTemporaryDirectory(), isDirectory: true)
        .appendingPathComponent("macsync-tests-\(UUID().uuidString)", isDirectory: true)
    try? FileManager.default.createDirectory(at: url, withIntermediateDirectories: true)
    return url
}

private func touchSnapshot(_ dest: URL, _ name: String) throws {
    let folder = dest.appendingPathComponent(name).appendingPathComponent("sub")
    try FileManager.default.createDirectory(at: folder, withIntermediateDirectories: true)
    try Data(name.utf8).write(to: folder.appendingPathComponent("file.txt"))
}
