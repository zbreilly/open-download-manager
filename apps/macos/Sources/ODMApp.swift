import AppKit
import SwiftUI
import UniformTypeIdentifiers
import UserNotifications

final class AppDelegate: NSObject, NSApplicationDelegate {
    func applicationDidFinishLaunching(_ notification: Notification) {
        // show notifications as banners even while the app is frontmost
        UNUserNotificationCenter.current().delegate = NotificationDelegate.shared
        Task { @MainActor in
            AppStore.shared.start()
            ClipboardMonitor.shared.start()
            Notifier.requestAuthorization()
        }
    }

    // Keep downloading in the background when the last window closes.
    func applicationShouldTerminateAfterLastWindowClosed(_ sender: NSApplication) -> Bool { false }

    func applicationShouldHandleReopen(_ sender: NSApplication, hasVisibleWindows flag: Bool) -> Bool {
        if !flag { Task { @MainActor in AppStore.shared.openMain() } }
        return true
    }

    func applicationWillTerminate(_ notification: Notification) {
        Engine.shared.shutdown()
    }

    // odm://add?url=… and plain http(s) links handed to the app
    func application(_ application: NSApplication, open urls: [URL]) {
        Task { @MainActor in
            for u in urls { DeepLink.handle(u) }
        }
    }
}

/// odm:// links and http(s) links handed to the app come from web pages, so
/// they open the Add window as source "web": only the URL is used (a
/// supplied name or referrer is ignored), the URL is not probed until the
/// user asks, and Start is not the default button.
enum DeepLink {
    @MainActor
    static func handle(_ u: URL) {
        let store = AppStore.shared
        switch u.scheme?.lowercased() {
        case "odm":
            let items = URLComponents(url: u, resolvingAgainstBaseURL: false)?.queryItems ?? []
            let q = Dictionary(items.map { ($0.name, $0.value ?? "") }, uniquingKeysWith: { a, _ in a })
            if u.host == "add" || u.host == nil {
                let target = (q["url"] ?? "").trimmingCharacters(in: .whitespacesAndNewlines)
                store.openAdd(AddParams(url: AddDownloadView.isHTTP(target) ? target : "", source: "web"))
            } else {
                store.openMain()
            }
        case "http", "https":
            store.openAdd(AddParams(url: u.absoluteString, source: "web"))
        default:
            break
        }
    }
}

/// Presents notifications while the app is frontmost (the default is to
/// drop them silently).
final class NotificationDelegate: NSObject, UNUserNotificationCenterDelegate, @unchecked Sendable {
    static let shared = NotificationDelegate()

    @objc(userNotificationCenter:willPresentNotification:withCompletionHandler:)
    func userNotificationCenter(_ center: UNUserNotificationCenter,
                                willPresent notification: UNNotification,
                                withCompletionHandler completionHandler: @escaping (UNNotificationPresentationOptions) -> Void) {
        completionHandler([.banner, .sound, .list])
    }
}

/// Routing for list shortcuts that clash with text editing (⌘⌫, ⌘.).
@MainActor
enum KeyRouting {
    /// A text field or text view is being edited in the key window.
    static var textHasFocus: Bool { NSApp.keyWindow?.firstResponder is NSText }

    static var mainWindowIsKey: Bool { NSApp.keyWindow?.identifier?.rawValue.hasPrefix("main") == true }

    /// For a menu item whose shortcut clashes with text editing: when it was
    /// triggered by its key equivalent while text is being edited (or outside
    /// the main window), pass the key's standard editing action (delete to
    /// line start, cancel) to the focused view instead. Chosen with the mouse,
    /// it always runs.
    static func listCommand(otherwise fallback: Selector, _ action: () -> Void) {
        let mouseTypes: [NSEvent.EventType] = [.leftMouseDown, .leftMouseUp, .rightMouseDown, .rightMouseUp, .otherMouseUp]
        let fromMouse = NSApp.currentEvent.map { mouseTypes.contains($0.type) } ?? false
        if fromMouse || (mainWindowIsKey && !textHasFocus) {
            action()
        } else {
            NSApp.sendAction(fallback, to: nil, from: nil)
        }
    }
}

/// Captures the SwiftUI openWindow action so non-view code (engine
/// events, the menu bar, the dock) can open windows.
struct WindowOpenerCapture: ViewModifier {
    @Environment(\.openWindow) private var openWindow

    func body(content: Content) -> some View {
        content.onAppear {
            let ow = openWindow
            AppStore.shared.openWindowAction = { kind, params, id in
                switch kind {
                case "add": ow(id: "add", value: params ?? AddParams())
                case "progress": if let id = id { ow(id: "progress", value: id) }
                default: ow(id: kind)
                }
            }
        }
    }
}

@main
struct OpenDownloadManagerApp: App {
    @NSApplicationDelegateAdaptor(AppDelegate.self) private var delegate
    @StateObject private var store = AppStore.shared

    var body: some Scene {
        Window("Open Download Manager", id: "main") {
            MainView()
                .environmentObject(store)
                .modifier(WindowOpenerCapture())
                .frame(minWidth: 900, minHeight: 560)
        }
        .defaultSize(width: 1280, height: 820)
        .commands { AppCommands(store: store) }

        WindowGroup("Add Download", id: "add", for: AddParams.self) { $params in
            AddDownloadView(params: params ?? AddParams())
                .environmentObject(store)
                .modifier(WindowOpenerCapture())
        }
        .defaultSize(width: 560, height: 680)
        .windowResizability(.contentMinSize)
        .handlesExternalEvents(matching: [])

        WindowGroup("Download", id: "progress", for: String.self) { $id in
            DownloadWindowView(id: id ?? "")
                .environmentObject(store)
        }
        .defaultSize(width: 760, height: 660)
        .handlesExternalEvents(matching: [])

        Window("Queues & Scheduler", id: "scheduler") {
            SchedulerView()
                .environmentObject(store)
                .frame(minWidth: 720, minHeight: 540)
        }
        .defaultSize(width: 820, height: 640)

        Window("Site Grabber", id: "grabber") {
            GrabberView()
                .environmentObject(store)
                .frame(minWidth: 680, minHeight: 500)
        }
        .defaultSize(width: 880, height: 640)

        Settings {
            SettingsView()
                .environmentObject(store)
        }

        MenuBarExtra {
            MenuBarContent()
                .environmentObject(store)
                .modifier(WindowOpenerCapture())
        } label: {
            MenuBarLabel(store: store)
        }
    }
}

struct MenuBarLabel: View {
    @ObservedObject var store: AppStore

    var body: some View {
        let showSpeed = (store.settings?.traySpeed ?? false) && store.totals.active > 0
        HStack(spacing: 4) {
            Image(nsImage: TrayIcons.image(store.trayState))
            if showSpeed { Text(Fmt.speed(store.totals.speedBps)).monospacedDigit() }
        }
    }
}

enum TrayIcons {
    static var cache: [String: NSImage] = [:]

    static func image(_ state: String) -> NSImage {
        if let i = cache[state] { return i }
        let name = "tray-\(state)Template"
        let img: NSImage
        if let url = Bundle.main.url(forResource: name, withExtension: "png"),
           let i = NSImage(contentsOf: url) {
            img = i
            img.size = NSSize(width: 18, height: 18)
        } else {
            img = NSImage(systemSymbolName: "arrow.down.circle", accessibilityDescription: "Open Download Manager") ?? NSImage()
        }
        img.isTemplate = true
        cache[state] = img
        return img
    }
}

struct MenuBarContent: View {
    @EnvironmentObject var store: AppStore

    var body: some View {
        if let err = store.startError {
            Text("Engine error: \(err)")
        }
        Text(store.totals.active > 0 ? "\(store.totals.active) active · \(Fmt.speed(store.totals.speedBps))" : "No active downloads")
        Divider()
        Button("Show Open Download Manager") { store.openMain() }
        Button("Add URL…") { store.openAdd() }
        Button("Site Grabber…") { store.openWindowAction?("grabber", nil, nil) }
        Divider()
        Button("Start Main Queue") { store.startQueue("main") }
        Button("Pause All") { store.pauseAll() }
        Toggle("Speed Limiter", isOn: Binding(
            get: { store.settings?.globalLimitEnabled ?? false },
            set: { _ in store.toggleLimit() }))
        Divider()
        Button("Scheduler…") { store.openScheduler() }
        SettingsButton()
        Divider()
        Button("Quit Open Download Manager") { NSApp.terminate(nil) }
            .keyboardShortcut("q")
    }
}

/// Opens the Settings window on macOS 13 and later.
struct SettingsButton: View {
    var title = "Settings…"
    var body: some View {
        if #available(macOS 14.0, *) {
            SettingsLink { Text(title) }
        } else {
            Button(title) {
                NSApp.activate(ignoringOtherApps: true)
                NSApp.sendAction(Selector(("showSettingsWindow:")), to: nil, from: nil)
            }
        }
    }
}

struct AppCommands: Commands {
    @ObservedObject var store: AppStore

    private var selected: [Download] { store.selection.compactMap { store.download($0) } }

    var body: some Commands {
        CommandGroup(replacing: .newItem) {
            Button("Add URL…") { store.openAdd() }
                .keyboardShortcut("n")
            Button("Add Batch Download…") { store.openAdd(AddParams(batch: true)) }
                .keyboardShortcut("n", modifiers: [.command, .shift])
            Button("Site Grabber…") { store.openWindowAction?("grabber", nil, nil) }
                .keyboardShortcut("g", modifiers: [.command, .shift])
        }
        CommandMenu("Downloads") {
            Button("Resume") { store.resume(selected.map(\.id)) }
                .keyboardShortcut("r")
                .disabled(!selected.contains { [.paused, .error, .queued].contains($0.status) })
            Button("Pause") {
                KeyRouting.listCommand(otherwise: Selector(("cancelOperation:"))) { store.pause(selected.map(\.id)) }
            }
                .keyboardShortcut(".")
                .disabled(!selected.contains { $0.status.isActive || $0.status == .queued })
            Button("Pause All") { store.pauseAll() }
                .keyboardShortcut(".", modifiers: [.command, .shift])
            Divider()
            Button("Open") { if let d = selected.first { store.openFile(d) } }
                .keyboardShortcut("o")
                .disabled(selected.first?.status != .complete)
            Button("Show in Finder") { if let d = selected.first { store.revealInFinder(d) } }
                .keyboardShortcut("r", modifiers: [.command, .shift])
                .disabled(selected.isEmpty)
            Button("Copy Link") { if let d = selected.first { store.copyLink(d) } }
                .keyboardShortcut("c", modifiers: [.command, .shift])
                .disabled(selected.isEmpty)
            Divider()
            Button("Delete…") {
                KeyRouting.listCommand(otherwise: Selector(("deleteToBeginningOfLine:"))) {
                    NotificationCenter.default.post(name: .odmAskDelete, object: nil)
                }
            }
                .keyboardShortcut(.delete)
                .disabled(selected.isEmpty)
            Button("Delete Completed") { store.deleteCompleted() }
            Divider()
            Button("Start Main Queue") { store.startQueue("main") }
            Button("Stop Main Queue") { store.stopQueue("main") }
            Button("Scheduler…") { store.openScheduler() }
                .keyboardShortcut("j")
            Divider()
            Toggle("Speed Limiter", isOn: Binding(
                get: { store.settings?.globalLimitEnabled ?? false },
                set: { _ in store.toggleLimit() }))
                .keyboardShortcut("l", modifiers: [.command, .option])
        }
        CommandGroup(after: .importExport) {
            Button("Import Link List…") { ImportExport.importList() }
            Button("Export Link List…") { ImportExport.exportList() }
        }
    }
}

extension Notification.Name {
    static let odmAskDelete = Notification.Name("odmAskDelete")
}

enum ImportExport {
    @MainActor
    static func importList() {
        let p = NSOpenPanel()
        p.allowedContentTypes = [.plainText, .commaSeparatedText]
        p.allowsMultipleSelection = false
        guard p.runModal() == .OK, let url = p.url else { return }
        Task { @MainActor in
            do {
                let urls: [String] = try await Engine.shared.callAsync("read_url_list", ["path": url.path])
                if urls.isEmpty {
                    AppStore.shared.report("No http(s) links found in that file.")
                } else {
                    AppStore.shared.openLinks(LinkParams(links: urls.map { .init(url: $0) }))
                }
            } catch {
                AppStore.shared.report(error.localizedDescription)
            }
        }
    }

    @MainActor
    static func exportList() {
        let p = NSSavePanel()
        p.nameFieldStringValue = "downloads.txt"
        p.allowedContentTypes = [.plainText]
        guard p.runModal() == .OK, let url = p.url else { return }
        Engine.shared.runAsync("export_list", ["path": url.path, "onlyUnfinished": false]) { AppStore.shared.report($0) }
    }
}
