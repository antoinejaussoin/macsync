import Foundation

struct TOMLError: LocalizedError, Equatable {
    var message: String
    init(_ message: String) { self.message = message }
    var errorDescription: String? { message }
    var description: String { message }
}

enum TOMLValue: Equatable {
    case string(String)
    case int(Int64)
    case bool(Bool)
    case array([TOMLValue])
    case table(TOMLTable)
}

final class TOMLTable: Equatable {
    var values: [String: TOMLValue] = [:]
    /// True once a `[header]` explicitly opened this table. Implicit parents may be opened once.
    var explicit = false

    static func == (lhs: TOMLTable, rhs: TOMLTable) -> Bool {
        lhs.values == rhs.values
    }

    static func parse(_ text: String) throws -> TOMLTable {
        try TOMLParser(text).parse()
    }

    func rejectUnknown(in allowed: Set<String>) throws {
        for key in values.keys where !allowed.contains(key) {
            throw TOMLError("unknown key \"\(key)\"")
        }
    }

    func optionalString(_ key: String) throws -> String? {
        guard let value = values[key] else { return nil }
        guard case .string(let string) = value else { throw TOMLError("\"\(key)\" must be a string") }
        return string
    }

    func optionalBool(_ key: String) throws -> Bool? {
        guard let value = values[key] else { return nil }
        guard case .bool(let flag) = value else { throw TOMLError("\"\(key)\" must be true or false") }
        return flag
    }

    func optionalInt(_ key: String) throws -> Int? {
        guard let value = values[key] else { return nil }
        guard case .int(let number) = value else { throw TOMLError("\"\(key)\" must be an integer") }
        return Int(number)
    }

    func optionalUInt(_ key: String) throws -> UInt64? {
        guard let value = values[key] else { return nil }
        guard case .int(let number) = value, number >= 0 else {
            throw TOMLError("\"\(key)\" must be an integer")
        }
        return UInt64(number)
    }

    func optionalDate(_ key: String) throws -> Date? {
        guard let text = try optionalString(key) else { return nil }
        guard let date = parseRFC3339(text) else {
            throw TOMLError("\"\(key)\" is not a timestamp (\(text))")
        }
        return date
    }

    func optionalTable(_ key: String) throws -> TOMLTable? {
        guard let value = values[key] else { return nil }
        guard case .table(let table) = value else { throw TOMLError("\"\(key)\" must be a table") }
        return table
    }

    func array(_ key: String) throws -> [TOMLValue] {
        guard let value = values[key] else { throw TOMLError("missing \"\(key)\"") }
        guard case .array(let items) = value else { throw TOMLError("\"\(key)\" must be an array") }
        return items
    }

    func stringArray(_ key: String) throws -> [String] {
        try array(key).map { value in
            guard case .string(let string) = value else { throw TOMLError("\"\(key)\" must be an array of strings") }
            return string
        }
    }
}

func tomlString(_ value: String) -> String {
    var out = "\""
    for scalar in value.unicodeScalars {
        switch scalar {
        case "\\": out += "\\\\"
        case "\"": out += "\\\""
        case "\n": out += "\\n"
        case "\r": out += "\\r"
        case "\t": out += "\\t"
        default:
            if scalar.value < 0x20 {
                out += String(format: "\\u%04X", scalar.value)
            } else {
                out.unicodeScalars.append(scalar)
            }
        }
    }
    out += "\""
    return out
}

func tomlKey(_ key: String) -> String {
    let bare = key.allSatisfy { $0.isLetter || $0.isNumber || $0 == "_" || $0 == "-" }
    return bare && !key.isEmpty ? key : tomlString(key)
}

func tomlStringArray(_ values: [String]) -> String {
    let encoded = values.map(tomlString)
    if encoded.count <= 1 {
        return "[\(encoded.joined(separator: ", "))]"
    }
    return "[\n" + encoded.map { "    \($0)," }.joined(separator: "\n") + "\n]"
}

private final class TOMLParser {
    private let chars: [Character]
    private var index = 0
    private var line = 1

    init(_ text: String) {
        chars = Array(text)
    }

    func parse() throws -> TOMLTable {
        let root = TOMLTable()
        var current = root
        if chars.first == "\u{feff}" { index = 1 }
        while true {
            skipTrivia()
            if peek() == nil { break }
            if peek() == "[" {
                current = try parseHeader(root: root)
            } else {
                let path = try parseKeyPath()
                skipSpace()
                guard bump() == "=" else { throw error("expected =") }
                let value = try parseValue()
                try assign(path, value, in: current)
            }
        }
        return root
    }

    private func parseHeader(root: TOMLTable) throws -> TOMLTable {
        guard bump() == "[" else { throw error("expected [") }
        let isArray = peek() == "["
        if isArray { _ = bump() }
        skipSpace()
        let path = try parseKeyPath()
        skipSpace()
        guard bump() == "]" else { throw error("expected ]") }
        if isArray {
            guard bump() == "]" else { throw error("expected ]]") }
        }
        return try defineTable(root: root, path: path, array: isArray)
    }

    private func defineTable(root: TOMLTable, path: [String], array: Bool) throws -> TOMLTable {
        guard let last = path.last else { throw error("empty table name") }
        var cursor = root
        for key in path.dropLast() {
            cursor = try descend(cursor, key: key)
        }
        if array {
            let table = TOMLTable()
            table.explicit = true
            if let existing = cursor.values[last] {
                guard case .array(var items) = existing else {
                    throw error("\"\(last)\" is not an array of tables")
                }
                items.append(.table(table))
                cursor.values[last] = .array(items)
            } else {
                cursor.values[last] = .array([.table(table)])
            }
            return table
        }
        if let existing = cursor.values[last] {
            guard case .table(let table) = existing, !table.explicit else {
                throw error("table \"\(path.joined(separator: "."))\" is already defined")
            }
            table.explicit = true
            return table
        }
        let table = TOMLTable()
        table.explicit = true
        cursor.values[last] = .table(table)
        return table
    }

    /// Walk into a table, or into the last element of an array of tables.
    private func descend(_ table: TOMLTable, key: String) throws -> TOMLTable {
        if let existing = table.values[key] {
            switch existing {
            case .table(let child):
                return child
            case .array(let items):
                guard let last = items.last, case .table(let child) = last else {
                    throw error("\"\(key)\" has no table to open")
                }
                return child
            default:
                throw error("\"\(key)\" is not a table")
            }
        }
        let child = TOMLTable()
        table.values[key] = .table(child)
        return child
    }

    private func assign(_ path: [String], _ value: TOMLValue, in table: TOMLTable) throws {
        guard let last = path.last else { return }
        var cursor = table
        for key in path.dropLast() {
            if let existing = cursor.values[key] {
                guard case .table(let child) = existing else { throw error("\"\(key)\" is not a table") }
                cursor = child
            } else {
                let child = TOMLTable()
                cursor.values[key] = .table(child)
                cursor = child
            }
        }
        if cursor.values[last] != nil { throw error("duplicate key \"\(last)\"") }
        cursor.values[last] = value
    }

    private func parseKeyPath() throws -> [String] {
        var keys = [try parseKey()]
        while true {
            skipSpace()
            if peek() == "." {
                _ = bump()
                keys.append(try parseKey())
            } else {
                break
            }
        }
        return keys
    }

    private func parseKey() throws -> String {
        skipSpace()
        if peek() == "\"" {
            _ = bump()
            return try parseBasicString()
        }
        if peek() == "'" {
            _ = bump()
            return try parseLiteralString()
        }
        var key = ""
        while let character = peek(), character.isLetter || character.isNumber || character == "_" || character == "-" {
            key.append(bump()!)
        }
        if key.isEmpty { throw error("expected a key") }
        return key
    }

    private func parseValue() throws -> TOMLValue {
        skipSpace()
        if peek() == "\"" {
            _ = bump()
            if peek() == "\"", lookahead(2) == "\"" {
                _ = bump(); _ = bump()
                return .string(try parseMultilineBasic())
            }
            return .string(try parseBasicString())
        }
        if peek() == "'" {
            _ = bump()
            if peek() == "'", lookahead(2) == "'" {
                _ = bump(); _ = bump()
                return .string(try parseMultilineLiteral())
            }
            return .string(try parseLiteralString())
        }
        if peek() == "[" {
            _ = bump()
            return .array(try parseArray())
        }
        if peek() == "{" {
            _ = bump()
            return .table(try parseInlineTable())
        }
        if consumeWord("true") { return .bool(true) }
        if consumeWord("false") { return .bool(false) }
        return try parseNumberOrDate()
    }

    private func parseArray() throws -> [TOMLValue] {
        var items: [TOMLValue] = []
        while true {
            skipTrivia()
            if peek() == nil { throw error("unclosed array") }
            if peek() == "]" {
                _ = bump()
                return items
            }
            items.append(try parseValue())
            skipTrivia()
            if peek() == "," {
                _ = bump()
                continue
            }
            if peek() == "]" {
                _ = bump()
                return items
            }
            throw error("expected , or ]")
        }
    }

    private func parseInlineTable() throws -> TOMLTable {
        let table = TOMLTable()
        table.explicit = true
        while true {
            skipTrivia()
            if peek() == "}" {
                _ = bump()
                return table
            }
            let path = try parseKeyPath()
            skipSpace()
            guard bump() == "=" else { throw error("expected =") }
            let value = try parseValue()
            try assign(path, value, in: table)
            skipTrivia()
            if peek() == "," {
                _ = bump()
                continue
            }
            if peek() == "}" {
                _ = bump()
                return table
            }
            throw error("expected , or }")
        }
    }

    private func parseBasicString() throws -> String {
        var out = ""
        while let character = bump() {
            if character == "\"" { return out }
            if character == "\n" || character == "\r" { throw error("newline in string") }
            if character == "\\" {
                out.append(contentsOf: try parseEscape(multiline: false))
            } else {
                out.append(character)
            }
        }
        throw error("unterminated string")
    }

    private func parseLiteralString() throws -> String {
        var out = ""
        while let character = bump() {
            if character == "'" { return out }
            if character == "\n" || character == "\r" { throw error("newline in string") }
            out.append(character)
        }
        throw error("unterminated string")
    }

    private func parseMultilineBasic() throws -> String {
        if peek() == "\r" { _ = bump() }
        if peek() == "\n" { _ = bump() }
        var out = ""
        while peek() != nil {
            if peek() == "\"", lookahead(2) == "\"", lookahead(3) == "\"" {
                _ = bump(); _ = bump(); _ = bump()
                return out
            }
            let character = bump()!
            if character == "\\" {
                out.append(contentsOf: try parseEscape(multiline: true))
            } else {
                out.append(character)
            }
        }
        throw error("unterminated string")
    }

    private func parseMultilineLiteral() throws -> String {
        if peek() == "\r" { _ = bump() }
        if peek() == "\n" { _ = bump() }
        var out = ""
        while peek() != nil {
            if peek() == "'", lookahead(2) == "'", lookahead(3) == "'" {
                _ = bump(); _ = bump(); _ = bump()
                return out
            }
            out.append(bump()!)
        }
        throw error("unterminated string")
    }

    private func parseEscape(multiline: Bool) throws -> String {
        guard let character = bump() else { throw error("truncated escape") }
        switch character {
        case "b": return "\u{8}"
        case "t": return "\t"
        case "n": return "\n"
        case "f": return "\u{c}"
        case "r": return "\r"
        case "\"": return "\""
        case "\\": return "\\"
        case "u": return try unicodeEscape(digits: 4)
        case "U": return try unicodeEscape(digits: 8)
        case "\n", "\r":
            guard multiline else { throw error("bad escape") }
            if character == "\r", peek() == "\n" { _ = bump() }
            while let next = peek(), next == " " || next == "\t" || next == "\n" || next == "\r" {
                _ = bump()
            }
            return ""
        default:
            throw error("bad escape")
        }
    }

    private func unicodeEscape(digits: Int) throws -> String {
        var hex = ""
        for _ in 0 ..< digits {
            guard let character = bump(), character.isHexDigit else { throw error("bad unicode escape") }
            hex.append(character)
        }
        guard let value = UInt32(hex, radix: 16), let scalar = UnicodeScalar(value) else {
            throw error("bad unicode escape")
        }
        return String(Character(scalar))
    }

    private func parseNumberOrDate() throws -> TOMLValue {
        let start = index
        if peek() == "+" || peek() == "-" { _ = bump() }
        guard let first = peek(), first.isNumber else { throw error("expected a value") }
        while let character = peek() {
            if character.isNumber || character == "_" || character == "-" || character == ":"
                || character == "T" || character == "t" || character == "Z" || character == "z" || character == "+"
            {
                _ = bump()
                continue
            }
            break
        }
        let token = String(chars[start ..< index])
        if token.contains("-") || token.contains(":") || token.contains("T") || token.contains("t") || token.contains("Z") {
            return .string(token)
        }
        let digits = token.replacingOccurrences(of: "_", with: "")
        if digits.hasPrefix("0x") || digits.hasPrefix("0o") || digits.hasPrefix("0b") || digits.hasPrefix("+0x") {
            throw TOMLError("unsupported integer \(token) on line \(line)")
        }
        guard let number = Int64(digits) else { throw error("invalid integer \(token)") }
        return .int(number)
    }

    private func consumeWord(_ word: String) -> Bool {
        let wordChars = Array(word)
        guard index + wordChars.count <= chars.count else { return false }
        for offset in 0 ..< wordChars.count where chars[index + offset] != wordChars[offset] {
            return false
        }
        let end = index + wordChars.count
        if end < chars.count {
            let next = chars[end]
            if next.isLetter || next.isNumber || next == "_" || next == "-" { return false }
        }
        index = end
        return true
    }

    private func skipTrivia() {
        while true {
            skipSpace()
            if peek() == "\n" || peek() == "\r" {
                _ = bump()
                continue
            }
            if peek() == "#" {
                while let character = peek(), character != "\n" { _ = bump() }
                continue
            }
            break
        }
    }

    private func skipSpace() {
        while peek() == " " || peek() == "\t" { _ = bump() }
    }

    private func peek() -> Character? {
        index < chars.count ? chars[index] : nil
    }

    private func lookahead(_ offset: Int) -> Character? {
        let position = index + offset - 1
        return position < chars.count ? chars[position] : nil
    }

    @discardableResult
    private func bump() -> Character? {
        guard index < chars.count else { return nil }
        let character = chars[index]
        index += 1
        if character == "\n" { line += 1 }
        return character
    }

    private func error(_ message: String) -> TOMLError {
        TOMLError("\(message) on line \(line)")
    }
}
