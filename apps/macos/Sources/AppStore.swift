import AppKit
import SwiftUI
import UserNotifications

/// App-wide observable state, fed by engine events.
@MainActor
final class AppStore: ObservableObject {
    static let shared = AppStore()

    @Published var downloads: [Download] = []
    @Published var queues: [DownloadQueue] = []
    @Published var settings: AppSettings?
    @Published var totals = Totals()
    @Published var ext = ExtensionStatus(connected: false, browsers: [])
    @Published var selection = Set<String>()
    @Published var errorMessage: String?
    @Published var power: PowerCountdown?
    @Published var pendingLinks: LinkParams?
    @Published var recentComplete: Date?
    @Published var startError: String?

    private var index: [String: Int] = [:]

    /// Bumped when list membership or a row's status/category/queue/name
    /// changes (anything filters, counts or non-progress sorts depend on).
    private(set) var listVersion = 0
    /// Bumped on every progress tick.
    private(set) var progressVersion = 0
    private var countsCache: (version: Int, counts: [SidebarItem: Int])?

    /// Keeps idle sleep off while downloads are active.
    private var keepAwake: NSObjectProtocol?

    /// Window opener captured from a SwiftUI view (openWindow environment).
    var openWindowAction: ((String, AddParams?, String?) -> Void)?

    struct PowerCountdown: Equatable {
        var action: String
        var queue: String
        var deadline: Date
        var token = UUID()
    }

    // MARK: - startup

    func start() {
        let dir = FileManager.default.urls(for: .applicationSupportDirectory, in: .userDomainMask)[0]
            .appendingPathComponent("app.opendm.desktop", isDirectory: true)
        Engine.shared.onEvent = { name, data in
            Task { @MainActor in AppStore.shared.handle(name, data) }
        }
        do {
            try Engine.shared.start(dataDir: dir)
        } catch {
            startError = error.localizedDescription
            return
        }
        reload()
        syncLoginItem()
        applyAppearance()
    }

    func reload() {
        let e = Engine.shared
        if let list: [Download] = try? e.call("list_downloads") { setDownloads(list) }
        if let q: [DownloadQueue] = try? e.call("list_queues") { queues = q }
        if let s: AppSettings = try? e.call("get_settings") { settings = s }
        if let t: Totals = try? e.call("totals") { totals = t; updateKeepAwake() }
        if let x: ExtensionStatus = try? e.call("extension_status") { ext = x }
    }

    private func setDownloads(_ list: [Download]) {
        listVersion &+= 1
        downloads = list
        reindex()
    }

    private func reindex() {
        index.removeAll(keepingCapacity: true)
        for (i, d) in downloads.enumerated() { index[d.id] = i }
    }

    /// O(1) lookup through the id → position index.
    func download(_ id: String) -> Download? {
        guard let i = index[id], i < downloads.count, downloads[i].id == id else { return nil }
        return downloads[i]
    }

    /// Sidebar badge counts, recomputed only when listVersion changes.
    func sidebarCounts() -> [SidebarItem: Int] {
        if let c = countsCache, c.version == listVersion { return c.counts }
        var c: [SidebarItem: Int] = [:]
        for d in downloads {
            c[.library("all"), default: 0] += 1
            if d.status.isActive { c[.library("active"), default: 0] += 1 }
            if d.status == .complete {
                c[.library("finished"), default: 0] += 1
            } else {
                c[.library("unfinished"), default: 0] += 1
            }
            c[.category(d.category), default: 0] += 1
            if let q = d.queueId { c[.queue(q), default: 0] += 1 }
        }
        countsCache = (listVersion, c)
        return c
    }

    func queueName(_ id: String?) -> String {
        guard let id = id else { return "—" }
        return queues.first(where: { $0.id == id })?.name ?? "—"
    }

    // MARK: - events

    private func upsert(_ d: Download) {
        listVersion &+= 1
        if let i = index[d.id], i < downloads.count {
            var nd = d
            // keep the locally accumulated speed history if the record has none
            if nd.speedHistory.isEmpty { nd.speedHistory = downloads[i].speedHistory }
            downloads[i] = nd
        } else {
            downloads.append(d)
            index[d.id] = downloads.count - 1
        }
    }

    func handle(_ name: String, _ data: Data) {
        let e = Engine.shared
        switch name {
        case "progress_batch":
            guard let items = e.decode([ProgressItem].self, data) else { return }
            var list = downloads
            var statusChanged = false
            for p in items {
                guard let i = index[p.id], i < list.count else { continue }
                if list[i].status != p.status { statusChanged = true }
                list[i].receivedBytes = p.receivedBytes
                if let t = p.totalBytes { list[i].totalBytes = t }
                list[i].speedBps = p.speedBps
                list[i].etaSecs = p.etaSecs
                list[i].activeConnections = p.activeConnections
                list[i].segments = p.segments
                list[i].status = p.status
            }
            progressVersion &+= 1
            if statusChanged { listVersion &+= 1 }
            downloads = list
        case "download_changed", "download_added":
            if let d = e.decode(Download.self, data) { upsert(d) }
        case "download_removed":
            if let id = e.decode(String.self, data) {
                if selection.contains(id) { selection.remove(id) }
                if let i = index[id], i < downloads.count {
                    listVersion &+= 1
                    downloads.remove(at: i)
                    reindex()
                }
            }
        case "totals":
            if let t = e.decode(Totals.self, data) {
                totals = t
                var list = downloads
                var changed = false
                for i in list.indices where list[i].status == .downloading || list[i].status == .connecting {
                    list[i].speedHistory.append(list[i].speedBps)
                    if list[i].speedHistory.count > 60 { list[i].speedHistory.removeFirst(list[i].speedHistory.count - 60) }
                    changed = true
                }
                if changed { downloads = list }
                updateDockBadge()
                updateKeepAwake()
            }
        case "queues_changed":
            if let q = e.decode([DownloadQueue].self, data) { queues = q }
        case "settings_changed":
            if let s = e.decode(AppSettings.self, data) {
                settings = s
                applyAppearance()
            }
        case "extension_status":
            if let x = e.decode(ExtensionStatus.self, data), x != ext { ext = x }
        case "download_completed":
            if let d = e.decode(Download.self, data) { completed(d) }
        case "download_failed":
            if let d = e.decode(Download.self, data), settings?.notifyFailure ?? true {
                Notifier.post(title: "Download failed: \(d.fileName)", body: d.error ?? "Unknown error", id: d.id)
            }
        case "queue_finished":
            if let q = e.decode(DownloadQueue.self, data) { queueFinished(q) }
        case "show_add":
            if let p = e.decode(AddParams.self, data) { openAdd(p) }
        case "show_links":
            if let p = e.decode(LinkParams.self, data) { openLinks(p) }
        default:
            break
        }
    }

    private func completed(_ d: Download) {
        recentComplete = Date()
        let path = d.fullPath
        switch d.whenFinished {
        case "openFile": NSWorkspace.shared.open(URL(fileURLWithPath: path))
        case "openFolder": NSWorkspace.shared.activateFileViewerSelecting([URL(fileURLWithPath: path)])
        case "none": break
        default:
            if settings?.notifyComplete ?? true {
                Notifier.post(title: "Download complete", body: d.fileName, id: d.id)
            }
        }
        if settings?.showCompleteDialog == true { openProgress(d.id) }
    }

    private func queueFinished(_ q: DownloadQueue) {
        if settings?.notifyQueueFinished ?? true {
            Notifier.post(title: "Queue finished", body: "\(q.name) has no more files to download", id: "q-\(q.id)")
        }
        guard ["sleep", "shutdown", "exit"].contains(q.onFinish) else { return }
        let pc = PowerCountdown(action: q.onFinish, queue: q.name, deadline: Date().addingTimeInterval(60))
        power = pc
        openMain()
        Task { @MainActor in
            try? await Task.sleep(nanoseconds: 60_000_000_000)
            guard let cur = AppStore.shared.power, cur.token == pc.token else { return }
            AppStore.shared.power = nil
            Power.perform(pc.action)
        }
    }

    // MARK: - windows

    func openAdd(_ p: AddParams = AddParams()) {
        NSApp.activate(ignoringOtherApps: true)
        // let a window that is being brought forward (e.g. by a URL event)
        // settle first, so the Add window ends up on top
        Task { @MainActor in
            try? await Task.sleep(nanoseconds: 150_000_000)
            AppStore.shared.openWindowAction?("add", p, nil)
        }
    }

    func openProgress(_ id: String) {
        NSApp.activate(ignoringOtherApps: true)
        openWindowAction?("progress", nil, id)
    }

    func openLinks(_ p: LinkParams) {
        pendingLinks = p
        NSApp.activate(ignoringOtherApps: true)
        openWindowAction?("grabber", nil, nil)
    }

    func openMain() {
        NSApp.activate(ignoringOtherApps: true)
        if let w = NSApp.windows.first(where: { $0.identifier?.rawValue.hasPrefix("main") == true }) {
            w.makeKeyAndOrderFront(nil)
        } else {
            openWindowAction?("main", nil, nil)
        }
    }

    func openScheduler() {
        NSApp.activate(ignoringOtherApps: true)
        openWindowAction?("scheduler", nil, nil)
    }

    // MARK: - actions

    func report(_ msg: String) { errorMessage = msg }

    func act(_ method: String, _ args: [String: Any] = [:]) {
        Engine.shared.runAsync(method, args) { [weak self] msg in self?.report(msg) }
    }

    func resume(_ ids: [String]) { act("resume", ["ids": ids]) }
    func pause(_ ids: [String]) { act("pause", ["ids": ids]) }
    func pauseAll() { act("pause_all") }
    func delete(_ ids: [String], files: Bool) {
        act("delete", ["ids": ids, "deleteFiles": files])
        for id in ids { selection.remove(id) }
    }
    func deleteCompleted() { act("delete_completed") }
    func redownload(_ id: String) { act("redownload", ["id": id]) }
    func startQueue(_ id: String) { act("start_queue", ["id": id]) }
    func stopQueue(_ id: String) { act("stop_queue", ["id": id]) }
    func moveToQueue(_ ids: [String], _ q: String?) { act("move_to_queue", ["ids": ids, "queueId": orNull(q)]) }
    func setCategory(_ ids: [String], _ c: String) { act("set_category", ["ids": ids, "category": c]) }

    func assignExtension(_ ext: String, to category: String) {
        act("assign_extensions", ["category": category, "extensions": [ext]])
        let same = downloads.filter { $0.fileExtension == ext && $0.category != category }.map(\.id)
        if !same.isEmpty { setCategory(same, category) }
    }

    func setGlobalLimit(_ bps: UInt64?) {
        act("set_global_limit", ["bps": orNull(bps)])
    }

    func toggleLimit() {
        guard let s = settings else { return }
        setGlobalLimit(s.globalLimitEnabled ? nil : s.globalLimitBps)
    }

    func saveSettings(_ new: AppSettings) {
        var s = new
        if settings?.launchAtLogin != s.launchAtLogin {
            // keep the setting equal to what the system actually did
            let r = LoginItem.set(s.launchAtLogin)
            s.launchAtLogin = r.isOn
            if let m = r.message { report(m) }
        }
        settings = s
        let v = Engine.shared.jsonValue(s)
        act("save_settings", ["settings": v])
        applyAppearance()
    }

    /// Bring the stored "open at login" setting in line with the system's
    /// login items (the user can change them in System Settings).
    private func syncLoginItem() {
        guard var s = settings else { return }
        let real = LoginItem.isOn
        guard s.launchAtLogin != real else { return }
        s.launchAtLogin = real
        settings = s
        act("save_settings", ["settings": Engine.shared.jsonValue(s)])
    }

    /// Save a queue's settings. Without `order` in the payload the engine
    /// keeps the queue's current order; send the order only after the user
    /// reordered (reorder_queue). Throws the engine's error.
    func saveQueue(_ q: DownloadQueue) async throws -> DownloadQueue {
        var v = Engine.shared.jsonValue(q)
        if var dict = v as? [String: Any] {
            dict.removeValue(forKey: "order")
            v = dict
        }
        return try await Engine.shared.callAsync("save_queue", ["queue": v])
    }

    func openFile(_ d: Download) {
        let url = URL(fileURLWithPath: d.fullPath)
        if FileManager.default.fileExists(atPath: url.path) {
            NSWorkspace.shared.open(url)
        } else {
            report("The file is no longer there. It may have been moved or deleted.")
        }
    }

    func revealInFinder(_ d: Download) {
        let fm = FileManager.default
        let full = d.fullPath
        let part = full + ".odmpart"
        if fm.fileExists(atPath: full) {
            NSWorkspace.shared.activateFileViewerSelecting([URL(fileURLWithPath: full)])
        } else if fm.fileExists(atPath: part) {
            NSWorkspace.shared.activateFileViewerSelecting([URL(fileURLWithPath: part)])
        } else {
            NSWorkspace.shared.open(URL(fileURLWithPath: d.saveDir, isDirectory: true))
        }
    }

    func copyLink(_ d: Download) {
        NSPasteboard.general.clearContents()
        NSPasteboard.general.setString(d.url, forType: .string)
    }

    func cancelPower() { power = nil }

    // MARK: - appearance, dock

    func applyAppearance() {
        switch settings?.theme ?? "dark" {
        case "light": NSApp.appearance = NSAppearance(named: .aqua)
        case "system": NSApp.appearance = nil
        default: NSApp.appearance = NSAppearance(named: .darkAqua)
        }
    }

    private func updateDockBadge() {
        NSApp.dockTile.badgeLabel = totals.active > 0 ? "\(totals.active)" : nil
    }

    /// Hold a ProcessInfo activity (no idle system sleep) while anything is
    /// downloading; release it when nothing is.
    private func updateKeepAwake() {
        let busy = totals.active > 0
        if busy, keepAwake == nil {
            keepAwake = ProcessInfo.processInfo.beginActivity(
                options: [.idleSystemSleepDisabled, .userInitiated],
                reason: "Downloading files")
        } else if !busy, let a = keepAwake {
            ProcessInfo.processInfo.endActivity(a)
            keepAwake = nil
        }
    }

    /// Menu-bar icon state, by the brand priority rules.
    var trayState: String {
        let recent = (recentComplete.map { Date().timeIntervalSince($0) < 5 }) ?? false
        if totals.failed > 0 && totals.active == 0 && !recent { return "error" }
        if totals.active > 0 && totals.limitOn { return "limited" }
        if totals.active > 0 { return "downloading" }
        if recent { return "complete" }
        if totals.paused > 0 { return "paused" }
        return "idle"
    }
}

enum Notifier {
    static func requestAuthorization() {
        UNUserNotificationCenter.current().requestAuthorization(options: [.alert, .sound]) { _, _ in }
    }

    static func post(title: String, body: String, id: String) {
        let c = UNMutableNotificationContent()
        c.title = title
        c.body = body
        let req = UNNotificationRequest(identifier: id + "-" + UUID().uuidString, content: c, trigger: nil)
        UNUserNotificationCenter.current().add(req, withCompletionHandler: nil)
    }
}

/// JSON null for nil, the value otherwise.
func orNull<T>(_ v: T?) -> Any {
    if let v = v { return v }
    return NSNull()
}

/// JSON null for an empty string.
func orNull(_ s: String) -> Any {
    s.isEmpty ? NSNull() as Any : s as Any
}
