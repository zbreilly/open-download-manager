import Foundation

// JSON model shared with the Rust engine (camelCase keys).

enum DownloadStatus: String, Codable, CaseIterable {
    case queued, connecting, downloading, paused, assembling, complete, error

    var isActive: Bool { self == .connecting || self == .downloading || self == .assembling }
}

enum SegState: String, Codable {
    case idle, connecting, receiving, reconnecting, done
}

struct Segment: Codable, Hashable {
    var start: UInt64
    var end: UInt64
    var received: UInt64
    var state: SegState
    var speedBps: UInt64?

    /// end == u64::MAX means "until EOF"
    var knownEnd: Bool { end < UInt64.max / 2 }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        start = try c.decode(UInt64.self, forKey: .start)
        received = try c.decode(UInt64.self, forKey: .received)
        state = try c.decode(SegState.self, forKey: .state)
        speedBps = try c.decodeIfPresent(UInt64.self, forKey: .speedBps)
        if let e = try? c.decode(UInt64.self, forKey: .end) {
            end = e
        } else {
            let d = try c.decode(Double.self, forKey: .end)
            end = d >= 1.8e19 ? UInt64.max : UInt64(max(0, d))
        }
    }
}

struct Checksum: Codable, Hashable {
    var algo: String
    var expected: String
}

struct Download: Codable, Identifiable, Hashable {
    var id: String
    var url: String
    var finalUrl: String?
    var fileName: String
    var ext: String
    var category: String
    var queueId: String?
    var saveDir: String
    var totalBytes: UInt64?
    var receivedBytes: UInt64
    var speedBps: UInt64
    var etaSecs: UInt64?
    var status: DownloadStatus
    var scheduledAt: String?
    var resumable: Bool
    var maxConnections: UInt32
    var activeConnections: UInt32
    var segments: [Segment]
    var speedHistory: [UInt64]
    var error: String?
    var addedAt: String
    var completedAt: String?
    var mime: String?
    var `protocol`: String?
    var referrer: String?
    var checksum: Checksum?
    var checksumOk: Bool?
    var speedLimitBps: UInt64?
    var whenFinished: String
    var manual: Bool
    var description: String?

    // Sort keys for the table
    var sizeSort: UInt64 { totalBytes ?? 0 }
    var progressSort: Double {
        if status == .complete { return 1 }
        guard let t = totalBytes, t > 0 else { return 0 }
        return Double(receivedBytes) / Double(t)
    }
    var speedSort: UInt64 { status.isActive ? speedBps : 0 }
    var etaSort: UInt64 { status == .downloading ? (etaSecs ?? .max) : .max }
    var statusSort: String { status.rawValue }
    var queueSort: String { queueId ?? "" }
    var fileExtension: String {
        let e = (fileName as NSString).pathExtension.lowercased()
        return e.count <= 10 ? e : ""
    }
    var fullPath: String { (saveDir as NSString).appendingPathComponent(fileName) }
}

struct ProgressItem: Codable {
    var id: String
    var receivedBytes: UInt64
    var totalBytes: UInt64?
    var speedBps: UInt64
    var etaSecs: UInt64?
    var activeConnections: UInt32
    var segments: [Segment]
    var status: DownloadStatus
}

struct Totals: Codable, Equatable {
    var speedBps: UInt64 = 0
    var active: UInt32 = 0
    var paused: UInt32 = 0
    var queued: UInt32 = 0
    var scheduled: UInt32 = 0
    var failed: UInt32 = 0
    var complete: UInt32 = 0
    var freeBytes: UInt64?
    var limitOn: Bool = false
    var limitBps: UInt64 = 0
}

struct DownloadQueue: Codable, Identifiable, Hashable {
    var id: String
    var name: String
    var builtin: Bool
    var maxConcurrent: UInt32
    var retryFailed: UInt32
    var startEnabled: Bool
    var startTime: String
    var stopEnabled: Bool
    var stopTime: String
    var days: [Bool]
    var onFinish: String
    var order: [String]
    var running: Bool
    var nextRun: String?

    static func new(name: String) -> DownloadQueue {
        DownloadQueue(id: "", name: name, builtin: false, maxConcurrent: 2, retryFailed: 3,
                      startEnabled: false, startTime: "01:00", stopEnabled: false, stopTime: "07:00",
                      days: Array(repeating: true, count: 7), onFinish: "none", order: [], running: false, nextRun: nil)
    }
}

struct Category: Codable, Identifiable, Hashable {
    var id: String
    var name: String
    var icon: String
    var extensions: [String]
    var folder: String?
}

struct AppSettings: Codable, Equatable {
    var launchAtLogin: Bool
    var minimizeToTray: Bool
    var clipboardMonitor: Bool
    var showProgressOnStart: Bool
    var showCompleteDialog: Bool
    var confirmDelete: Bool
    var defaultConnections: UInt32
    var maxSimultaneous: UInt32
    var globalLimitEnabled: Bool
    var globalLimitBps: UInt64
    var perDownloadLimitEnabled: Bool
    var perDownloadLimitBps: UInt64
    var limitOnStartup: Bool
    var captureEnabled: Bool
    var captureExtensions: [String]
    var bypassKey: String
    var capturePausedUntil: Int64?
    var captureMinSizeKb: UInt64
    var extensionPort: UInt16
    var categories: [Category]
    var defaultFolder: String
    var tempFolder: String?
    var proxyMode: String
    var proxyHost: String
    var proxyPort: UInt16
    var proxyUser: String
    var proxyPass: String
    var notifyComplete: Bool
    var notifyFailure: Bool
    var notifyQueueFinished: Bool
    var theme: String
    var density: String
    var traySpeed: Bool
    var userAgent: String
    var maxRetries: UInt32
    var retryDelaySecs: UInt64
    var timeoutSecs: UInt64
    var minSegmentKb: UInt64
    var verifyTls: Bool
}

struct ProbeResult: Codable {
    var finalUrl: String
    var fileName: String
    var totalBytes: UInt64?
    var mime: String?
    var resumable: Bool
    var `protocol`: String
    var category: String
}

struct AddRequest: Codable {
    var url: String
    var fileName: String?
    var saveDir: String?
    var category: String?
    var queueId: String?
    var start: Bool
    var connections: UInt32?
    var referrer: String?
    var cookies: String?
    var userAgent: String?
    var headers: [[String]] = []
    var username: String?
    var password: String?
    var checksum: Checksum?
    var onExists: String?
    var rememberFolder: Bool = false
    var totalBytes: UInt64?
    var resumable: Bool?
    var mime: String?
}

struct GrabbedLink: Codable, Identifiable, Hashable {
    var url: String
    var fileName: String
    var ext: String
    var category: String
    var source: String
    var id: String { url }
}

struct FinalPath: Codable {
    var path: String
    var exists: Bool
    var partPath: String
    var partExists: Bool
    var dir: String
}

struct ExtensionStatus: Codable, Equatable {
    var connected: Bool
    var browsers: [String]
    /// why the browser bridge isn't listening (nil while it is)
    var bridgeError: String? = nil
}

/// `bridge_error` event payload (the event is `null` once it listens again).
struct BridgeErrorEvent: Codable {
    var port: Int
    var error: String
}

/// Parameters for an Add Download window (also used for extension handoff).
/// The engine's `show_add` payload has only url/fileName/referrer/cookies/
/// userAgent/totalBytes/mime (any may be null), so decoding fills defaults
/// for everything else (see the extension below).
struct AddParams: Codable, Hashable {
    var url: String = ""
    var fileName: String?
    var referrer: String?
    var cookies: String?
    var userAgent: String?
    var totalBytes: UInt64?
    var mime: String?
    var batch: Bool = false
    /// "web" when a web page asked for this (odm:// or a link handed to the
    /// app): no automatic probe, no default Start button, and only the URL is
    /// taken from the request.
    var source: String?
    var nonce: String = UUID().uuidString

    var fromWeb: Bool { source == "web" }
}

// In an extension so the memberwise initializer stays available.
extension AddParams {
    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        url = try c.decodeIfPresent(String.self, forKey: .url) ?? ""
        fileName = try c.decodeIfPresent(String.self, forKey: .fileName)
        referrer = try c.decodeIfPresent(String.self, forKey: .referrer)
        cookies = try c.decodeIfPresent(String.self, forKey: .cookies)
        userAgent = try c.decodeIfPresent(String.self, forKey: .userAgent)
        totalBytes = try? c.decodeIfPresent(UInt64.self, forKey: .totalBytes)
        mime = try c.decodeIfPresent(String.self, forKey: .mime)
        batch = (try? c.decodeIfPresent(Bool.self, forKey: .batch)) ?? false
        source = try c.decodeIfPresent(String.self, forKey: .source)
        nonce = try c.decodeIfPresent(String.self, forKey: .nonce) ?? UUID().uuidString
    }
}

/// `show_links` payload: `{ links: [{ url, cookies? }], referrer?, userAgent? }`.
/// Cookies belong to each link and are never shared between links.
struct LinkParams: Codable, Hashable {
    struct Link: Codable, Hashable {
        var url: String
        var cookies: String?

        init(url: String, cookies: String? = nil) {
            self.url = url
            self.cookies = cookies
        }
    }
    var links: [Link] = []
    var referrer: String?
    var userAgent: String?
}

extension LinkParams {
    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        links = try c.decodeIfPresent([Link].self, forKey: .links) ?? []
        referrer = try c.decodeIfPresent(String.self, forKey: .referrer)
        userAgent = try c.decodeIfPresent(String.self, forKey: .userAgent)
    }
}
