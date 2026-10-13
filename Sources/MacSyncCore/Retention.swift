import Foundation

/// Snapshot names are local times like `2026-10-11T081300`.
let stampFormatLength = 17

struct SnapshotStamp: Comparable, Equatable {
    var year: Int
    var month: Int
    var day: Int
    var hour: Int
    var minute: Int
    var second: Int
    var date: Date

    static func < (lhs: SnapshotStamp, rhs: SnapshotStamp) -> Bool {
        if lhs.year != rhs.year { return lhs.year < rhs.year }
        if lhs.month != rhs.month { return lhs.month < rhs.month }
        if lhs.day != rhs.day { return lhs.day < rhs.day }
        if lhs.hour != rhs.hour { return lhs.hour < rhs.hour }
        if lhs.minute != rhs.minute { return lhs.minute < rhs.minute }
        return lhs.second < rhs.second
    }
}

func formatStamp(_ date: Date) -> String {
    let parts = Calendar.current.dateComponents([.year, .month, .day, .hour, .minute, .second], from: date)
    return String(
        format: "%04d-%02d-%02dT%02d%02d%02d",
        parts.year ?? 0,
        parts.month ?? 0,
        parts.day ?? 0,
        parts.hour ?? 0,
        parts.minute ?? 0,
        parts.second ?? 0
    )
}

func parseStamp(_ name: String) -> SnapshotStamp? {
    guard name.count == stampFormatLength else { return nil }
    let chars = Array(name)
    guard chars[4] == "-", chars[7] == "-", chars[10] == "T" else { return nil }
    func number(_ from: Int, _ count: Int) -> Int? {
        Int(String(chars[from ..< (from + count)]))
    }
    guard
        let year = number(0, 4),
        let month = number(5, 2),
        let day = number(8, 2),
        let hour = number(11, 2),
        let minute = number(13, 2),
        let second = number(15, 2)
    else { return nil }
    var components = DateComponents()
    components.year = year
    components.month = month
    components.day = day
    components.hour = hour
    components.minute = minute
    components.second = second
    var calendar = Calendar(identifier: .gregorian)
    calendar.timeZone = .current
    guard let date = calendar.date(from: components) else { return nil }
    let roundTrip = calendar.dateComponents([.year, .month, .day, .hour, .minute, .second], from: date)
    guard roundTrip.year == year, roundTrip.month == month, roundTrip.day == day,
          roundTrip.hour == hour, roundTrip.minute == minute, roundTrip.second == second
    else { return nil }
    return SnapshotStamp(year: year, month: month, day: day, hour: hour, minute: minute, second: second, date: date)
}

struct Retention: Equatable {
    var keepLast: Int
    var keepDaily: Int
    var keepWeekly: Int
    var keepMonthly: Int
    var keepYearly: KeepYearly

    static let `default` = Retention(keepLast: 30, keepDaily: 0, keepWeekly: 0, keepMonthly: 3, keepYearly: .forever)
}

enum KeepYearly: Equatable {
    case forever
    case count(Int)

    var limit: Int {
        switch self {
        case .forever: return Int.max
        case .count(let count): return count
        }
    }
}

enum Tier: String, Equatable {
    case latest, last, daily, weekly, monthly, yearly

    var label: String { rawValue }
}

struct Decision: Equatable {
    var name: String
    var time: SnapshotStamp
    var keptBy: [Tier]

    var keep: Bool { !keptBy.isEmpty }
}

/// Decides which snapshots survive, newest first.
///
/// Each calendar tier walks newest to oldest and keeps the newest snapshot of
/// each of its N most recent buckets. A bucket already covered by another tier
/// still counts, so it is not stored twice. The snapshot `latest` points at is
/// always kept.
func retentionPlan(snapshots: [(String, SnapshotStamp)], policy: Retention, latest: String?) -> [Decision] {
    let sorted = snapshots.sorted { lhs, rhs in
        if lhs.1 != rhs.1 { return lhs.1 > rhs.1 }
        return lhs.0 > rhs.0
    }
    var kept: [Int: [Tier]] = [:]
    func keep(_ index: Int, _ tier: Tier) {
        kept[index, default: []].append(tier)
    }
    for (index, snapshot) in sorted.enumerated() where latest == snapshot.0 {
        keep(index, .latest)
    }
    let lastCount = min(sorted.count, max(0, policy.keepLast))
    for index in 0 ..< lastCount {
        keep(index, .last)
    }

    let tiers: [(Tier, Int, (SnapshotStamp) -> String)] = [
        (.daily, policy.keepDaily, { "\($0.year)-\($0.month)-\($0.day)" }),
        (.weekly, policy.keepWeekly, { stamp in
            let week = isoWeek(stamp)
            return "\(week.year)-\(week.week)"
        }),
        (.monthly, policy.keepMonthly, { "\($0.year)-\($0.month)" }),
        (.yearly, policy.keepYearly.limit, { "\($0.year)" }),
    ]
    for (tier, limit, bucket) in tiers {
        guard limit > 0 else { continue }
        var lastBucket: String?
        var bucketsSeen = 0
        for (index, snapshot) in sorted.enumerated() {
            if bucketsSeen >= limit { break }
            let value = bucket(snapshot.1)
            if lastBucket != value {
                lastBucket = value
                bucketsSeen += 1
                keep(index, tier)
            }
        }
    }
    return sorted.enumerated().map { index, snapshot in
        Decision(name: snapshot.0, time: snapshot.1, keptBy: kept[index] ?? [])
    }
}

func isoWeek(_ stamp: SnapshotStamp) -> (year: Int, week: Int) {
    var calendar = Calendar(identifier: .iso8601)
    calendar.timeZone = .current
    let week = calendar.component(.weekOfYear, from: stamp.date)
    let year = calendar.component(.yearForWeekOfYear, from: stamp.date)
    return (year, week)
}
