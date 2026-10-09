import SwiftUI

enum Fmt {
    static let KB: Double = 1024
    static let MB: Double = 1024 * 1024
    static let GB: Double = 1024 * 1024 * 1024

    static func bytes(_ n: UInt64?) -> String {
        guard let n = n else { return "—" }
        let d = Double(n)
        if d < MB { return String(format: "%.1f KB", d / KB) }
        if d < GB { return String(format: "%.1f MB", d / MB) }
        return String(format: "%.2f GB", d / GB)
    }

    static func speed(_ bps: UInt64?) -> String {
        guard let b = bps, b > 0 else { return "—" }
        return bytes(b) + "/s"
    }

    static func percent(_ d: Download) -> Int {
        if d.status == .complete { return 100 }
        guard let t = d.totalBytes, t > 0 else { return 0 }
        // clamp before converting: Int(Double) traps on huge values
        return Int(min(99, max(0, Double(d.receivedBytes) / Double(t) * 100)))
    }

    static func eta(_ s: UInt64?) -> String {
        guard let s = s, s < UInt64.max / 2 else { return "—" }
        if s < 60 { return "\(s)s" }
        if s < 3600 { return "\(s / 60)m \(String(format: "%02d", s % 60))s" }
        if s < 86400 { return "\(s / 3600)h \(String(format: "%02d", (s % 3600) / 60))m" }
        return "> 1 day"
    }

    private static let iso: ISO8601DateFormatter = {
        let f = ISO8601DateFormatter()
        f.formatOptions = [.withInternetDateTime, .withFractionalSeconds]
        return f
    }()
    private static let isoPlain = ISO8601DateFormatter()

    static func date(_ s: String?) -> Date? {
        guard let s = s else { return nil }
        return iso.date(from: s) ?? isoPlain.date(from: s)
    }

    static func added(_ s: String?) -> String {
        guard let d = date(s) else { return "—" }
        let cal = Calendar.current
        let t = DateFormatter()
        t.dateFormat = "HH:mm"
        if cal.isDateInToday(d) { return "Today \(t.string(from: d))" }
        if cal.isDateInYesterday(d) { return "Yesterday \(t.string(from: d))" }
        let f = DateFormatter()
        f.dateFormat = cal.isDate(d, equalTo: Date(), toGranularity: .year) ? "MMM d" : "MMM d, yyyy"
        return f.string(from: d)
    }

    static func hhmm(_ s: String?) -> String {
        guard let d = date(s) else { return "" }
        let f = DateFormatter()
        f.dateFormat = "HH:mm"
        return f.string(from: d)
    }

    static func statusLabel(_ d: Download) -> String {
        switch d.status {
        case .downloading: return "Downloading"
        case .paused: return "Paused"
        case .queued: return d.scheduledAt != nil ? "Starts \(hhmm(d.scheduledAt))" : "Queued"
        case .error: return "Failed"
        case .complete: return "Complete"
        case .connecting: return "Connecting"
        case .assembling: return "Finishing"
        }
    }

    static func statusColor(_ s: DownloadStatus) -> Color {
        switch s {
        case .downloading, .connecting, .assembling: return .accentColor
        case .paused: return .orange
        case .error: return .red
        case .complete: return .green
        case .queued: return .secondary
        }
    }

    static func mbps(_ bps: UInt64) -> Double { (Double(bps) / MB * 10).rounded() / 10 }

    /// Largest speed limit accepted from a text field, in MB/s.
    static let maxLimitMB: Double = 1_000_000
    static let limitError = "Enter a speed between 0 and 1,000,000 MB/s."

    /// MB/s → bytes/s, clamped to 0…maxLimitMB (NaN and negatives give 0).
    /// Never traps: UInt64(Double) crashes on non-finite or out-of-range values.
    static func bps(fromMB mb: Double) -> UInt64 {
        guard mb.isFinite, mb > 0 else { return 0 }
        return UInt64((min(mb, maxLimitMB) * MB).rounded())
    }

    /// Parse a "MB/s" text field. nil for anything that is not a finite
    /// number above 0 and at most maxLimitMB (so callers can show an error
    /// instead of silently turning a limit off).
    static func limitBps(_ text: String) -> UInt64? {
        guard let v = Double(text.trimmingCharacters(in: .whitespaces).replacingOccurrences(of: ",", with: ".")),
              v.isFinite, v > 0, v <= maxLimitMB else { return nil }
        return max(1, bps(fromMB: v))
    }

    static func host(_ url: String) -> String { URL(string: url)?.host ?? "" }
}
