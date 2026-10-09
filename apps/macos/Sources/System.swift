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
enum LoginItem {
    static func set(_ on: Bool) {
        do {
            if on {
                if SMAppService.mainApp.status != .enabled { try SMAppService.mainApp.register() }
            } else {
                if SMAppService.mainApp.status == .enabled { try SMAppService.mainApp.unregister() }
            }
        } catch {
            NSLog("login item: \(error.localizedDescription)")
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
