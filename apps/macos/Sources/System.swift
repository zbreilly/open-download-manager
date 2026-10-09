import AppKit
import ServiceManagement

/// Sleep / shut down / quit when a queue finishes.
enum Power {
    @MainActor
    static func perform(_ action: String) {
        switch action {
        case "exit":
            NSApp.terminate(nil)
        case "sleep":
            run("/usr/bin/pmset", ["sleepnow"])
        case "shutdown":
            Engine.shared.shutdown()
            run("/usr/bin/osascript", ["-e", "tell application \"System Events\" to shut down"])
        default:
            break
        }
    }

    private static func run(_ path: String, _ args: [String]) {
        let p = Process()
        p.executableURL = URL(fileURLWithPath: path)
        p.arguments = args
        try? p.run()
    }
}

/// Launch at login through the system's login items.
@MainActor
enum LoginItem {
    struct Outcome {
        /// What the system has now (pending approval counts as on: the item
        /// is registered and runs once the user allows it).
        var isOn: Bool
        /// Something to tell the user, if the request did not simply work.
        var message: String?
    }

    /// Whether the app is registered as a login item (enabled or awaiting approval).
    static var isOn: Bool {
        let st = SMAppService.mainApp.status
        return st == .enabled || st == .requiresApproval
    }

    /// Register or unregister, then read the status back.
    static func set(_ on: Bool) -> Outcome {
        let app = SMAppService.mainApp
        var failure: String?
        do {
            if on {
                if app.status != .enabled { try app.register() }
            } else if app.status == .enabled || app.status == .requiresApproval {
                try app.unregister()
            }
        } catch {
            failure = error.localizedDescription
            NSLog("login item: \(error.localizedDescription)")
        }
        switch app.status {
        case .enabled:
            if on { return Outcome(isOn: true, message: nil) }
            return Outcome(isOn: true, message: "Open Download Manager could not be removed from your login items"
                + (failure.map { " (\($0))." } ?? ".") + " You can remove it in System Settings › General › Login Items.")
        case .requiresApproval:
            if on {
                SMAppService.openSystemSettingsLoginItems()
                return Outcome(isOn: true, message: "To open Open Download Manager at login, allow it in System Settings › General › Login Items.")
            }
            return Outcome(isOn: true, message: "Open Download Manager is still listed in your login items. You can remove it in System Settings › General › Login Items.")
        default: // .notRegistered, .notFound
            if !on { return Outcome(isOn: false, message: nil) }
            return Outcome(isOn: false, message: "Open Download Manager could not be added to your login items"
                + (failure.map { " (\($0))." } ?? ".") + " You can add it in System Settings › General › Login Items.")
        }
    }
}

/// Offers copied download links (by file type) in the Add dialog.
@MainActor
final class ClipboardMonitor {
    static let shared = ClipboardMonitor()
    private var timer: Timer?
    private var lastCount = NSPasteboard.general.changeCount

    func start() {
        timer?.invalidate()
        timer = Timer.scheduledTimer(withTimeInterval: 1.0, repeats: true) { _ in
            Task { @MainActor in ClipboardMonitor.shared.tick() }
        }
    }

    private func tick() {
        let pb = NSPasteboard.general
        guard pb.changeCount != lastCount else { return }
        lastCount = pb.changeCount
        let store = AppStore.shared
        guard let s = store.settings, s.clipboardMonitor else { return }
        guard let text = pb.string(forType: .string)?.trimmingCharacters(in: .whitespacesAndNewlines),
              text.count < 4096, !text.contains(where: { $0.isWhitespace }),
              let url = URL(string: text), let scheme = url.scheme?.lowercased(),
              scheme == "http" || scheme == "https" else { return }
        let ext = url.pathExtension.lowercased()
        guard !ext.isEmpty, s.captureExtensions.contains(ext) else { return }
        if store.downloads.contains(where: { $0.url == text && $0.status != .error }) { return }
        store.openAdd(AddParams(url: text))
    }
}
