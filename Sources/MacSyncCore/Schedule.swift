import Foundation

struct ScheduleError: LocalizedError, Equatable {
    var message: String
    var errorDescription: String? { message }
}

/// Standard 5-field cron: minute hour day-of-month month day-of-week, Sunday = 0.
struct Cron {
    var minutes: Set<Int>
    var hours: Set<Int>
    var daysOfMonth: CronField
    var months: Set<Int>
    var daysOfWeek: CronField

    init(_ expression: String) throws {
        let trimmed = expression.trimmingCharacters(in: .whitespacesAndNewlines)
        let fields = trimmed.split(whereSeparator: \.isWhitespace).map(String.init)
        guard fields.count == 5 else {
            throw ScheduleError(message: "Invalid schedule \"\(trimmed)\": expected 5 fields (minute hour day month weekday)")
        }
        do {
            minutes = try Cron.parseField(fields[0], range: 0 ... 59, kind: .number)
            hours = try Cron.parseField(fields[1], range: 0 ... 23, kind: .number)
            daysOfMonth = try Cron.parseNamedField(fields[2], range: 1 ... 31, kind: .dayOfMonth)
            months = try Cron.parseField(fields[3], range: 1 ... 12, kind: .month)
            daysOfWeek = try Cron.parseNamedField(fields[4], range: 0 ... 7, kind: .weekday)
            if daysOfWeek.values.contains(7) {
                var values = daysOfWeek.values
                values.remove(7)
                values.insert(0)
                daysOfWeek.values = values
            }
        } catch let error as ScheduleError {
            throw error
        } catch {
            throw ScheduleError(message: "Invalid schedule \"\(trimmed)\": \(error)")
        }
    }
}

struct CronField {
    /// Bare `*` (or `?`). `*/2` is restricted, which matters for the day-of-month / weekday OR rule.
    var isStar: Bool
    var values: Set<Int>
}

private enum FieldKind {
    case number, month, dayOfMonth, weekday
}

extension Cron {
    fileprivate static func parseField(_ text: String, range: ClosedRange<Int>, kind: FieldKind) throws -> Set<Int> {
        try parseNamedField(text, range: range, kind: kind).values
    }

    fileprivate static func parseNamedField(_ text: String, range: ClosedRange<Int>, kind: FieldKind) throws -> CronField {
        let trimmed = text.trimmingCharacters(in: .whitespaces)
        if trimmed == "*" || trimmed == "?" {
            return CronField(isStar: true, values: Set(range))
        }
        var values = Set<Int>()
        for part in trimmed.split(separator: ",") {
            let piece = String(part)
            let stepSplit = piece.split(separator: "/", maxSplits: 1).map(String.init)
            let base = stepSplit[0]
            let step = try parseStep(stepSplit.count == 2 ? stepSplit[1] : nil)
            let span: ClosedRange<Int>
            if base == "*" || base == "?" {
                span = range
            } else if base.contains("-") {
                let ends = base.split(separator: "-", maxSplits: 1).map(String.init)
                guard ends.count == 2 else { throw fieldError(piece) }
                let lower = try parseToken(ends[0], range: range, kind: kind)
                let upper = try parseToken(ends[1], range: range, kind: kind)
                guard lower <= upper else { throw fieldError(piece) }
                span = lower ... upper
            } else {
                let value = try parseToken(base, range: range, kind: kind)
                span = value ... value
            }
            var cursor = span.lowerBound
            while cursor <= span.upperBound {
                values.insert(cursor)
                cursor += step
            }
        }
        if values.isEmpty { throw fieldError(trimmed) }
        let isStar = trimmed == "*" || trimmed == "?"
        return CronField(isStar: isStar, values: values)
    }

    private static func parseStep(_ text: String?) throws -> Int {
        guard let text else { return 1 }
        guard let step = Int(text), step > 0 else { throw fieldError(text) }
        return step
    }

    private static func parseToken(_ text: String, range: ClosedRange<Int>, kind: FieldKind) throws -> Int {
        if let value = Int(text) {
            guard range.contains(value) else { throw fieldError(text) }
            return value
        }
        let name = text.lowercased()
        let value: Int?
        switch kind {
        case .month:
            value = monthNames[name]
        case .weekday:
            value = weekdayNames[name]
        case .number, .dayOfMonth:
            value = nil
        }
        guard let value, range.contains(value) else { throw fieldError(text) }
        return value
    }

    private static func fieldError(_ text: String) -> ScheduleError {
        ScheduleError(message: "invalid field \"\(text)\"")
    }

    private static let monthNames = [
        "jan": 1, "feb": 2, "mar": 3, "apr": 4, "may": 5, "jun": 6,
        "jul": 7, "aug": 8, "sep": 9, "oct": 10, "nov": 11, "dec": 12,
    ]
    private static let weekdayNames = [
        "sun": 0, "mon": 1, "tue": 2, "wed": 3, "thu": 4, "fri": 5, "sat": 6,
    ]
}

func validateSchedule(_ expression: String) -> String? {
    let trimmed = expression.trimmingCharacters(in: .whitespacesAndNewlines)
    if trimmed.isEmpty { return nil }
    do {
        _ = try Cron(trimmed)
        return nil
    } catch {
        let message = (error as? LocalizedError)?.errorDescription ?? String(describing: error)
        if message.hasPrefix("Invalid schedule") { return message }
        return "Invalid schedule \"\(trimmed)\": \(message)"
    }
}

func nextAfter(_ expression: String, after: Date) -> Date? {
    let trimmed = expression.trimmingCharacters(in: .whitespacesAndNewlines)
    if trimmed.isEmpty { return nil }
    guard let cron = try? Cron(trimmed) else { return nil }
    return cron.next(after: after)
}

/// A job is due when a fire time has passed since the anchor. After sleep this
/// yields a single catch-up run, because the anchor moves to that run.
func scheduleIsDue(_ expression: String, anchor: Date, now: Date) -> Bool {
    guard let next = nextAfter(expression, after: anchor) else { return false }
    return next <= now
}

let weekdays = ["Sunday", "Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday"]

enum Preset: Equatable {
    case manual
    case hourly(minute: Int)
    case daily(hour: Int, minute: Int)
    case weekly(weekday: Int, hour: Int, minute: Int)
    case custom

    var label: String {
        switch self {
        case .manual: return "Manual only"
        case .hourly: return "Every hour"
        case .daily: return "Every day"
        case .weekly: return "Every week"
        case .custom: return "Custom (cron)"
        }
    }

    var kind: PresetKind {
        switch self {
        case .manual: return .manual
        case .hourly: return .hourly
        case .daily: return .daily
        case .weekly: return .weekly
        case .custom: return .custom
        }
    }

    func toCron() -> String? {
        switch self {
        case .manual: return ""
        case .hourly(let minute): return "\(minute) * * * *"
        case .daily(let hour, let minute): return "\(minute) \(hour) * * *"
        case .weekly(let weekday, let hour, let minute): return "\(minute) \(hour) * * \(weekday)"
        case .custom: return nil
        }
    }

    static func fromCron(_ expression: String) -> Preset {
        let fields = expression.split(whereSeparator: \.isWhitespace).map(String.init)
        if fields.isEmpty { return .manual }
        if fields.count != 5 { return .custom }
        func number(_ text: String, max: Int) -> Int? {
            guard let value = Int(text), value <= max else { return nil }
            return value
        }
        let minute = number(fields[0], max: 59)
        let hour = number(fields[1], max: 23)
        switch (minute, fields[1], hour, fields[2], fields[3], fields[4]) {
        case let (minute?, "*", _, "*", "*", "*"):
            return .hourly(minute: minute)
        case let (minute?, _, hour?, "*", "*", "*"):
            return .daily(hour: hour, minute: minute)
        case let (minute?, _, hour?, "*", "*", dow):
            if let weekday = number(dow, max: 7) {
                return .weekly(weekday: weekday % 7, hour: hour, minute: minute)
            }
            return .custom
        default:
            return .custom
        }
    }
}

enum PresetKind: String, CaseIterable, Identifiable {
    case manual, hourly, daily, weekly, custom
    var id: String { rawValue }
    var label: String {
        switch self {
        case .manual: return Preset.manual.label
        case .hourly: return Preset.hourly(minute: 0).label
        case .daily: return Preset.daily(hour: 2, minute: 0).label
        case .weekly: return Preset.weekly(weekday: 0, hour: 3, minute: 0).label
        case .custom: return Preset.custom.label
        }
    }

    func defaultPreset() -> Preset {
        switch self {
        case .manual: return .manual
        case .hourly: return .hourly(minute: 0)
        case .daily: return .daily(hour: 2, minute: 0)
        case .weekly: return .weekly(weekday: 0, hour: 3, minute: 0)
        case .custom: return .custom
        }
    }
}

extension Cron {
    /// Next fire time strictly after `after` (the minute `after` falls in does not count).
    func next(after: Date) -> Date? {
        let calendar = Calendar.current
        let truncated = calendar.date(from: calendar.dateComponents([.year, .month, .day, .hour, .minute], from: after))
        guard var cursor = truncated.flatMap({ calendar.date(byAdding: .minute, value: 1, to: $0) }) else { return nil }
        guard let limit = calendar.date(byAdding: .year, value: 5, to: cursor) else { return nil }
        while cursor < limit {
            let parts = calendar.dateComponents([.year, .month, .day, .hour, .minute, .weekday], from: cursor)
            guard let month = parts.month, let day = parts.day, let hour = parts.hour, let minute = parts.minute,
                  let weekday = parts.weekday
            else { return nil }
            let dow = weekday - 1
            if !months.contains(month) {
                guard let next = firstInstant(ofMonthAfter: cursor, calendar: calendar) else { return nil }
                cursor = next
                continue
            }
            if !dayMatches(day: day, weekday: dow) {
                guard let next = calendar.date(byAdding: .day, value: 1, to: calendar.startOfDay(for: cursor)) else { return nil }
                cursor = next
                continue
            }
            if !hours.contains(hour) {
                if let nextHour = hours.filter({ $0 > hour }).min(),
                   let next = setting(cursor, calendar: calendar, hour: nextHour, minute: minutes.min() ?? 0)
                {
                    cursor = next
                } else if let next = calendar.date(byAdding: .day, value: 1, to: calendar.startOfDay(for: cursor)) {
                    cursor = next
                } else {
                    return nil
                }
                continue
            }
            if !minutes.contains(minute) {
                if let nextMinute = minutes.filter({ $0 > minute }).min(),
                   let next = setting(cursor, calendar: calendar, hour: hour, minute: nextMinute)
                {
                    cursor = next
                } else if let hourStart = setting(cursor, calendar: calendar, hour: hour, minute: 0),
                          let next = calendar.date(byAdding: .hour, value: 1, to: hourStart)
                {
                    cursor = next
                } else {
                    return nil
                }
                continue
            }
            return cursor
        }
        return nil
    }

    /// Vixie cron: if both day-of-month and weekday are restricted, either one matching is enough.
    private func dayMatches(day: Int, weekday: Int) -> Bool {
        let dom = daysOfMonth.values.contains(day)
        let dow = daysOfWeek.values.contains(weekday)
        switch (daysOfMonth.isStar, daysOfWeek.isStar) {
        case (true, true): return true
        case (true, false): return dow
        case (false, true): return dom
        case (false, false): return dom || dow
        }
    }

    private func setting(_ date: Date, calendar: Calendar, hour: Int, minute: Int) -> Date? {
        var comps = calendar.dateComponents([.year, .month, .day], from: date)
        comps.hour = hour
        comps.minute = minute
        comps.second = 0
        return calendar.date(from: comps)
    }

    private func firstInstant(ofMonthAfter date: Date, calendar: Calendar) -> Date? {
        var comps = calendar.dateComponents([.year, .month], from: date)
        comps.month = (comps.month ?? 1) + 1
        comps.day = 1
        comps.hour = 0
        comps.minute = 0
        comps.second = 0
        return calendar.date(from: comps)
    }
}
