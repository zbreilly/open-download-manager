// Open Download Manager — native macOS shell (SwiftUI).
// Talks to the odm-serve control API on 127.0.0.1 (default :8080).
// Build: ./build.sh  →  build/OpenDM.app

import SwiftUI
import Combine

struct Job: Codable, Identifiable {
    let id: Int64
    let state: String
    let url: String
    let out: String
    var done: Int64?
    var total: Int64?

    var progress: Double? {
        guard let d = done, let t = total, t > 0 else { return nil }
        return Double(d) / Double(t)
    }
}

struct JobsResponse: Codable { let jobs: [Job] }

@MainActor
final class ODMConnection: ObservableObject {
    static let shared = ODMConnection()
    let baseURL: URL
    private(set) var token: String?

    init(port: UInt16 = 8901) {
        baseURL = URL(string: "http://127.0.0.1:\(port)")!
    }

    /// The API requires X-ODM-Token (same-origin injection protects the web
    /// UI; the native app fetches "/" once and extracts the injected token).
    func bootstrapToken() async {
        guard token == nil else { return }
        guard let html = try? await String(
            contentsOf: baseURL, encoding: .utf8) else { return }
        if let r = try? Regex(#"window\.ODM_TOKEN=\"([0-9a-f]+)\""#),
           let m = html.firstMatch(of: r) {
            token = String(m.output[1].substring ?? "")
        }
    }

    func request(_ path: String, method: String = "GET") -> URLRequest {
        var req = URLRequest(url: baseURL.appendingPathComponent(path))
        req.httpMethod = method
        if let t = token {
            req.setValue(t, forHTTPHeaderField: "X-ODM-Token")
        }
        return req
    }
}

@MainActor
final class JobListModel: ObservableObject {
    @Published var jobs: [Job] = []
    @Published var newURL = ""
    @Published var newOut = ""
    @Published var statusLine = ""
    let conn: ODMConnection

    init(port: UInt16 = 8901) {
        conn = ODMConnection(port: port)
    }

    func refresh() {
        Task {
            await conn.bootstrapToken()
            do {
                let (data, resp) = try await URLSession.shared.data(
                    for: conn.request("/api/jobs"))
                if let http = resp as? HTTPURLResponse, http.statusCode == 401 {
                    statusLine = "auth failed — token not accepted"
                    return
                }
                let decoded = try JSONDecoder().decode(JobsResponse.self, from: data)
                jobs = decoded.jobs
                statusLine = ""
            } catch {
                statusLine = "server unreachable: \(error.localizedDescription)"
            }
        }
    }

    func post(_ path: String) {
        Task {
            _ = try? await URLSession.shared.data(
                for: conn.request(path, method: "POST"))
            refresh()
        }
    }

    func add() {
        guard !newURL.isEmpty, !newOut.isEmpty else { return }
        var comps = URLComponents(
            url: conn.baseURL.appendingPathComponent("/api/add"),
            resolvingAgainstBaseURL: false)!
        comps.queryItems = [
            URLQueryItem(name: "url", value: newURL),
            URLQueryItem(name: "out", value: newOut),
        ]
        Task {
            var req = conn.request(comps.url!.absoluteString, method: "POST")
            req.url = comps.url!
            let result = try? await URLSession.shared.data(for: req)
            if result == nil { statusLine = "add failed" }
            newURL = ""
            newOut = ""
            refresh()
        }
    }
}

struct JobRow: View {
    let job: Job

    var stateColor: Color {
        switch job.state {
        case "done": return .green
        case "running": return .blue
        case "failed": return .red
        case "paused": return .orange
        default: return .secondary
        }
    }

    var body: some View {
        HStack {
            VStack(alignment: .leading, spacing: 2) {
                Text(job.url).font(.system(size: 12, weight: .medium))
                    .lineLimit(1)
                    .truncationMode(.middle)
                Text(job.out).font(.system(size: 10))
                    .foregroundColor(.secondary)
                    .lineLimit(1)
                    .truncationMode(.middle)
                if let p = job.progress {
                    ProgressView(value: p)
                        .frame(width: 180)
                }
            }
            Spacer()
            Text(job.state)
                .font(.system(size: 11, weight: .semibold))
                .foregroundColor(stateColor)
                .padding(.horizontal, 6)
                .padding(.vertical, 2)
                .background(stateColor.opacity(0.12))
                .cornerRadius(4)
            if job.state == "running" {
                Button("Pause") { postFor(job: job, action: "pause") }
                    .controlSize(.small)
            }
            if job.state == "paused" {
                Button("Resume") { postFor(job: job, action: "resume") }
                    .controlSize(.small)
            }
        }
        .padding(.vertical, 2)
    }

    func postFor(job: Job, action: String) {
        Task {
            let conn = ODMConnection.shared
            await conn.bootstrapToken()
            var req = conn.request("/api/\(action)?id=\(job.id)", method: "POST")
            req.url = URL(string: conn.baseURL.absoluteString
                + "/api/\(action)?id=\(job.id)")!
            _ = try? await URLSession.shared.data(for: req)
        }
    }
}

import SwiftUI
import WebKit
import Combine

@MainActor
final class WebControls: ObservableObject {
    static let shared = WebControls()
    var webView: WKWebView?

    func run(_ js: String) {
        webView?.evaluateJavaScript(js, completionHandler: nil)
    }
    func goBack() { webView?.goBack() }
    func goForward() { webView?.goForward() }
    func reload() { webView?.reload() }
}

struct ServerWebView: NSViewRepresentable {
    let url: URL

    func makeCoordinator() -> Coordinator { Coordinator() }

    class Coordinator: NSObject, WKUIDelegate {
        // window.open (Settings/Scheduler/Progress) -> load in this view
        func webView(
            _ webView: WKWebView,
            createWebViewWith configuration: WKWebViewConfiguration,
            for navigationAction: WKNavigationAction,
            windowFeatures: WKWindowFeatures
        ) -> WKWebView? {
            if navigationAction.targetFrame == nil {
                webView.load(navigationAction.request)
            }
            return nil
        }
    }

    func makeNSView(context: Context) -> WKWebView {
        let cfg = WKWebViewConfiguration()
        WKWebsiteDataStore.default().removeData(
            ofTypes: [WKWebsiteDataTypeDiskCache, WKWebsiteDataTypeMemoryCache],
            modifiedSince: .distantPast) {}
        let wv = WKWebView(frame: .zero, configuration: cfg)
        wv.uiDelegate = context.coordinator
        WebControls.shared.webView = wv
        // wait briefly for the bundled server, then load the design UI
        Task {
            for _ in 0..<50 {
                var req = URLRequest(url: url)
                req.httpMethod = "HEAD"
                req.timeoutInterval = 1
                let sem = DispatchSemaphore(value: 0)
                var up = false
                URLSession.shared.dataTask(with: req) { _, resp, _ in
                    if (resp as? HTTPURLResponse) != nil { up = true }
                    sem.signal()
                }.resume()
                _ = sem.wait(timeout: .now() + 1.5)
                if up { break }
                try? await Task.sleep(nanoseconds: 200_000_000)
            }
            await MainActor.run {
                wv.load(URLRequest(url: url))
            }
        }
        return wv
    }

    func updateNSView(_ wv: WKWebView, context: Context) {}
}

struct ContentView: View {
    @StateObject private var trayModel = TrayModel()
    let conn = ODMConnection.shared

    var body: some View {
        VStack(spacing: 0) {
            // native nav strip (matches tokens: surface bg, border)
            HStack(spacing: 6) {
                Button {
                    WebControls.shared.goBack()
                } label: {
                    Image(systemName: "chevron.left")
                }
                Button {
                    WebControls.shared.goForward()
                } label: {
                    Image(systemName: "chevron.right")
                }
                Button {
                    WebControls.shared.reload()
                } label: {
                    Image(systemName: "arrow.clockwise")
                }
                Spacer()
            }
            .padding(.horizontal, 10)
            .padding(.vertical, 5)
            .buttonStyle(.borderless)
            Divider()
            ServerWebView(url: conn.baseURL)
        }
        .frame(minWidth: 900, minHeight: 600)
        .onAppear { ServerManager.startBundledServerIfNeeded() }
    }
}

@MainActor
enum ServerManager {
    static var process: Process?

    /// Launch the bundled odm-serve (Contents/Resources/odm-serve) if no
    /// server is already listening on the port.
    static func startBundledServerIfNeeded() {
        let port = UInt16(ProcessInfo.processInfo.environment["ODM_PORT"] ?? "8901") ?? 8901
        DispatchQueue.global(qos: .utility).async {
            let sem = DispatchSemaphore(value: 0)
            var up = false
            var req = URLRequest(url: URL(string: "http://127.0.0.1:\(port)/")!)
            req.httpMethod = "HEAD"
            req.timeoutInterval = 1.5
            let task = URLSession.shared.dataTask(with: req) { _, resp, _ in
                if let http = resp as? HTTPURLResponse, http.statusCode > 0 {
                    up = true
                }
                sem.signal()
            }
            task.resume()
            _ = sem.wait(timeout: .now() + 3)
            if up { return }
            guard let exe = Bundle.main.path(forResource: "odm-serve", ofType: nil) else {
                return
            }
            let support = NSSearchPathForDirectoriesInDomains(
                .applicationSupportDirectory, .userDomainMask, true).first ?? NSTemporaryDirectory()
            let dir = (support as NSString).appendingPathComponent("OpenDM")
            try? FileManager.default.createDirectory(
                atPath: dir, withIntermediateDirectories: true)
            let jobs = (dir as NSString).appendingPathComponent("odm.jobs")
            let p = Process()
            p.executableURL = URL(fileURLWithPath: exe)
            p.arguments = ["--port", String(port), "--jobs", jobs, "--parts", "8"]
            try? p.run()
            DispatchQueue.main.async { self.process = p }
        }
    }
}

class AppDelegate: NSObject, NSApplicationDelegate {
    var minimizeToTray = false

    func applicationDidFinishLaunching(_ note: Notification) {
        // poll the setting so close-beavior reflects the UI toggle
        Timer.scheduledTimer(withTimeInterval: 5, repeats: true) { _ in
            let conn = ODMConnection.shared
            Task {
                await conn.bootstrapToken()
                guard let (data, _) = try? await URLSession.shared.data(
                    for: conn.request("/api/settings")) else { return }
                guard let obj = (try? JSONSerialization.jsonObject(
                    with: data)) as? [String: String],
                      let v = obj["minimizeToTray"] else { return }
                await MainActor.run { self.minimizeToTray = (v == "1") }
            }
        }
    }

    func applicationShouldTerminateAfterLastWindowClosed(_ sender: NSApplication) -> Bool {
        if minimizeToTray {
            NSApp.hide(nil)
            return false // keep running in the menu bar
        }
        return true
    }
}

@main
struct OpenDMApp: App {
    @NSApplicationDelegateAdaptor(AppDelegate.self) var appDelegate

    var body: some Scene {
        WindowGroup("Open Download Manager") {
            ContentView()
                .onAppear { ServerManager.startBundledServerIfNeeded() }
        }
        .defaultSize(width: 1280, height: 860)
        .commands {
            CommandGroup(after: .newItem) {
                Button("New Download…") {
                    WebControls.shared.run(
                        "document.getElementById('btn-add')?.click()")
                }
                .keyboardShortcut("n", modifiers: .command)
                Button("Start Pending Downloads") {
                    WebControls.shared.run(
                        "fetch('/api/run?max=4',{method:'POST'})")
                }
                .keyboardShortcut("r", modifiers: [.command, .shift])
            }
            CommandGroup(after: .toolbar) {
                Button("Back") { WebControls.shared.goBack() }
                    .keyboardShortcut("[", modifiers: .command)
                Button("Forward") { WebControls.shared.goForward() }
                    .keyboardShortcut("]", modifiers: .command)
                Button("Reload Page") { WebControls.shared.reload() }
                    .keyboardShortcut("r", modifiers: .command)
                Divider()
                Button("Open Web UI in Browser") {
                    if let url = URL(string: ODMConnection.shared.baseURL.absoluteString) {
                        NSWorkspace.shared.open(url)
                    }
                }
                .keyboardShortcut("l", modifiers: [.command, .shift])
            }
        }
        MenuBarExtra("Open Download Manager", systemImage: "arrow.down.circle") {
            Button("Open window") {
                NSApp.activate(ignoringOtherApps: true)
                for w in NSApp.windows where w.title.contains("Open Download Manager") {
                    w.makeKeyAndOrderFront(nil)
                }
            }
            Button("Pause all downloads") { TrayModel.shared.post("/api/pause-all") }
            Button("Run pending") { TrayModel.shared.post("/api/run") }
            Divider()
            Button("Quit") { NSApp.terminate(nil) }
        }
    }
}

@MainActor
final class TrayModel: ObservableObject {
    static let shared = TrayModel()
    func post(_ path: String) {
        let conn = ODMConnection.shared
        Task {
            await conn.bootstrapToken()
            URLSession.shared.dataTask(
                with: conn.request(path, method: "POST")).resume()
        }
    }
}
