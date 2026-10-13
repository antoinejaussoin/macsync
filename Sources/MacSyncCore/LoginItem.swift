import Foundation

let loginLabel = "com.macsync.agent"
let backgroundArgument = "--background"

func loginPlistURL() -> URL {
    URL(fileURLWithPath: Paths.home, isDirectory: true)
        .appendingPathComponent("Library/LaunchAgents", isDirectory: true)
        .appendingPathComponent("\(loginLabel).plist")
}

func loginItemEnabled() -> Bool {
    FileManager.default.fileExists(atPath: loginPlistURL().path)
}

func enableLoginItem() throws {
    let raw = Bundle.main.executableURL?.path ?? CommandLine.arguments[0]
    let executable = canonicalize(raw) ?? raw
    let path = loginPlistURL()
    let directory = path.deletingLastPathComponent()
    do {
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
    } catch {
        throw AppError("Could not create \(directory.path): \(error)")
    }
    do {
        try plistXML(executable: executable).write(to: path, atomically: true, encoding: .utf8)
    } catch {
        throw AppError("Could not write \(path.path): \(error)")
    }
}

func disableLoginItem() throws {
    do {
        try FileManager.default.removeItem(at: loginPlistURL())
    } catch let error as NSError {
        if error.domain == NSCocoaErrorDomain && error.code == NSFileNoSuchFileError { return }
        throw AppError("Could not remove the login item: \(error)")
    }
}

func runningFromBundle() -> Bool {
    let path = Bundle.main.executableURL?.path ?? CommandLine.arguments[0]
    return path.contains(".app/Contents/MacOS/")
}

func plistXML(executable: String) -> String {
    let escaped = xmlEscape(executable)
    return """
    <?xml version="1.0" encoding="UTF-8"?>
    <!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
    <plist version="1.0">
    <dict>
        <key>Label</key>
        <string>\(loginLabel)</string>
        <key>ProgramArguments</key>
        <array>
            <string>\(escaped)</string>
            <string>\(backgroundArgument)</string>
        </array>
        <key>RunAtLoad</key>
        <true/>
        <key>ProcessType</key>
        <string>Interactive</string>
        <key>LimitLoadToSessionType</key>
        <string>Aqua</string>
    </dict>
    </plist>
    """
}

func xmlEscape(_ text: String) -> String {
    text
        .replacingOccurrences(of: "&", with: "&amp;")
        .replacingOccurrences(of: "<", with: "&lt;")
        .replacingOccurrences(of: ">", with: "&gt;")
        .replacingOccurrences(of: "\"", with: "&quot;")
}
