import Foundation

enum AppLanguage: String, CaseIterable {
    case ru, en
    var label: String { self == .en ? "English" : "Русский" }
    var locale: Locale { Locale(identifier: self == .en ? "en_US" : "ru_RU") }
    static var current: AppLanguage { AppLanguage(rawValue: UserDefaults.standard.string(forKey: "language") ?? "ru") ?? .ru }
}

private enum Messages {
    static let english: [String: String] = {
        guard let url = Bundle.main.url(forResource: "en", withExtension: "json"),
              let data = try? Data(contentsOf: url),
              let values = try? JSONDecoder().decode([String: String].self, from: data) else { return [:] }
        return values
    }()
    static let placeholder = try! NSRegularExpression(pattern: #"\{(\d+)\}"#)
    static let patterns: [(NSRegularExpression, [Int], String)] = english.sorted { $0.key.replacingOccurrences(of: #"\{\d+\}"#, with: "", options: .regularExpression).count > $1.key.replacingOccurrences(of: #"\{\d+\}"#, with: "", options: .regularExpression).count }.compactMap { key, value in
        let matches = placeholder.matches(in: key, range: NSRange(key.startIndex..., in: key))
        guard !matches.isEmpty else { return nil }
        var pattern = "^", offset = key.startIndex, indices: [Int] = []
        for match in matches {
            guard let range = Range(match.range, in: key), let indexRange = Range(match.range(at: 1), in: key), let index = Int(key[indexRange]) else { return nil }
            pattern += NSRegularExpression.escapedPattern(for: String(key[offset..<range.lowerBound])) + "([\\s\\S]*?)"
            indices.append(index); offset = range.upperBound
        }
        pattern += NSRegularExpression.escapedPattern(for: String(key[offset...])) + "$"
        guard let expression = try? NSRegularExpression(pattern: pattern) else { return nil }
        return (expression, indices, value)
    }
}

func tr(_ source: String, _ args: Any...) -> String {
    let english = AppLanguage.current == .en
    var message = english ? Messages.english[source] ?? source : source
    var arguments = args.map { String(describing: $0) }
    if english && args.isEmpty && message == source && source.utf16.count <= 4096 && source.range(of: "[А-Яа-яЁё]", options: .regularExpression) != nil {
        for (expression, indices, value) in Messages.patterns {
            guard let match = expression.firstMatch(in: source, range: NSRange(source.startIndex..., in: source)) else { continue }
            arguments = Array(repeating: "", count: (indices.max() ?? 0) + 1)
            for (position, index) in indices.enumerated() {
                if let range = Range(match.range(at: position + 1), in: source) { arguments[index] = String(source[range]) }
            }
            message = value; break
        }
    }
    // Substitute from the end so argument text containing braces is never processed again.
    for match in Messages.placeholder.matches(in: message, range: NSRange(message.startIndex..., in: message)).reversed() {
        guard let indexRange = Range(match.range(at: 1), in: message), let index = Int(message[indexRange]),
              index < arguments.count, let range = Range(match.range, in: message) else { continue }
        message.replaceSubrange(range, with: arguments[index])
    }
    return message
}
