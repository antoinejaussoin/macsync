import Foundation

func formatBytes(_ bytes: UInt64) -> String {
    let units = ["B", "KB", "MB", "GB", "TB", "PB"]
    var value = Double(bytes)
    var unit = 0
    while value >= 1000, unit < units.count - 1 {
        value /= 1000
        unit += 1
    }
    if unit == 0 { return "\(bytes) B" }
    return String(format: "%.1f %@", locale: Locale(identifier: "en_US_POSIX"), value, units[unit])
}

func shortPath(_ path: String) -> String {
    let home = Paths.home
    let prefix = home.hasSuffix("/") ? home : home + "/"
    if path.hasPrefix(prefix) {
        let rest = String(path.dropFirst(prefix.count))
        if !rest.isEmpty { return "~/" + rest }
    }
    return path
}

func relativeTime(_ date: Date, now: Date) -> String {
    let secs = Int(now.timeIntervalSince(date))
    if secs < 0 { return relativeFuture(date, now: now) }
    switch secs {
    case 0 ... 59: return "just now"
    case 60 ... 3599: return "\(secs / 60) min ago"
    case 3600 ... 86_399: return "\(secs / 3600) h ago"
    case 86_400 ... 172_799: return "yesterday"
    default: return "\(secs / 86_400) days ago"
    }
}

func relativeFuture(_ date: Date, now: Date) -> String {
    let secs = Int(date.timeIntervalSince(now))
    let calendar = Calendar.current
    let when: String
    if calendar.isDate(date, inSameDayAs: now) {
        when = SelfDate.time.string(from: date)
    } else {
        when = SelfDate.weekday.string(from: date)
    }
    switch secs {
    case ...0: return "now"
    case 1 ... 3599: return "in \((secs + 59) / 60) min (\(when))"
    case 3600 ... 86_399: return "in \(secs / 3600) h (\(when))"
    default: return when
    }
}

func friendlyStamp(_ name: String) -> String {
    guard let date = parseStamp(name)?.date else { return name }
    return SelfDate.friendly.string(from: date)
}

enum SelfDate {
    static let time: DateFormatter = {
        let formatter = DateFormatter()
        formatter.locale = Locale(identifier: "en_US_POSIX")
        formatter.timeZone = .current
        formatter.dateFormat = "HH:mm"
        return formatter
    }()

    static let weekday: DateFormatter = {
        let formatter = DateFormatter()
        formatter.locale = Locale(identifier: "en_US_POSIX")
        formatter.dateFormat = "EEE d MMM HH:mm"
        return formatter
    }()

    static let friendly: DateFormatter = {
        let formatter = DateFormatter()
        formatter.locale = Locale(identifier: "en_US_POSIX")
        formatter.dateFormat = "EEE d MMM yyyy, HH:mm"
        return formatter
    }()

    static let logHeader: DateFormatter = {
        let formatter = DateFormatter()
        formatter.locale = Locale(identifier: "en_US_POSIX")
        formatter.dateFormat = "yyyy-MM-dd HH:mm:ss"
        return formatter
    }()

    static let rfc3339: ISO8601DateFormatter = {
        let formatter = ISO8601DateFormatter()
        formatter.formatOptions = [.withInternetDateTime]
        formatter.timeZone = .current
        return formatter
    }()

    static let rfc3339Fractional: ISO8601DateFormatter = {
        let formatter = ISO8601DateFormatter()
        formatter.formatOptions = [.withInternetDateTime, .withFractionalSeconds]
        return formatter
    }()
}

func formatRFC3339(_ date: Date) -> String {
    let fractional = date.timeIntervalSince1970.truncatingRemainder(dividingBy: 1)
    if abs(fractional) < 0.000_000_1 {
        return SelfDate.rfc3339.string(from: date)
    }
    return SelfDate.rfc3339Fractional.string(from: date)
}

func parseRFC3339(_ text: String) -> Date? {
    if let date = SelfDate.rfc3339Fractional.date(from: text) { return date }
    return SelfDate.rfc3339.date(from: text)
}

func textLines(_ text: String) -> [String] {
    if text.isEmpty { return [] }
    var parts = text.components(separatedBy: "\n")
    if text.hasSuffix("\n") { parts.removeLast() }
    return parts.map { $0.hasSuffix("\r") ? String($0.dropLast()) : $0 }
}
