import Foundation

private let ntfyBase = "https://ntfy.sh/"

func ntfyTopicURL(_ topic: String) -> URL? {
    var trimmed = topic.trimmingCharacters(in: .whitespacesAndNewlines)
    if trimmed.isEmpty { return nil }
    while trimmed.hasSuffix("/") { trimmed.removeLast() }
    if trimmed.hasPrefix("http://") || trimmed.hasPrefix("https://") {
        return URL(string: trimmed)
    }
    return URL(string: ntfyBase + trimmed)
}

func ntfyTestMessage() -> (title: String, body: String) {
    let host = Host.current().localizedName ?? Host.current().name ?? "Mac"
    return ("macsync test", "Notifications are working (from \(host)).")
}

/// Synchronous POST to ntfy. Returns an error message on failure.
func postNtfy(notify: NotifyConfig, title: String, body: String, tags: String = "warning") -> String? {
    guard notify.enabled, let url = ntfyTopicURL(notify.topic) else {
        return "notify.topic is empty"
    }
    var request = URLRequest(url: url, timeoutInterval: 15)
    request.httpMethod = "POST"
    request.setValue(title, forHTTPHeaderField: "Title")
    request.setValue(tags, forHTTPHeaderField: "Tags")
    let token = notify.token.trimmingCharacters(in: .whitespacesAndNewlines)
    if !token.isEmpty {
        request.setValue("Bearer \(token)", forHTTPHeaderField: "Authorization")
    }
    request.httpBody = body.data(using: .utf8)
    let semaphore = DispatchSemaphore(value: 0)
    var result: String?
    URLSession.shared.dataTask(with: request) { _, response, error in
        defer { semaphore.signal() }
        if let error {
            result = error.localizedDescription
            return
        }
        let status = (response as? HTTPURLResponse)?.statusCode ?? 0
        if !(200 ..< 300).contains(status) {
            result = "ntfy HTTP \(status)"
        }
    }.resume()
    _ = semaphore.wait(timeout: .now() + 20)
    return result
}

func shouldMountAlert(last: Date?, remindHours: Int, now: Date = Date()) -> Bool {
    guard let last else { return true }
    let hours = max(1, remindHours)
    return now.timeIntervalSince(last) >= Double(hours) * 3600
}
