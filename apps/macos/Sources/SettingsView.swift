import AppKit
import SwiftUI

/// Native Settings window. Changes apply as you make them.
struct SettingsView: View {
    @EnvironmentObject var store: AppStore
    @State private var draft: AppSettings?
    /// The engine's settings the draft is based on. A field whose draft value
    /// still equals this is untouched, so changes made elsewhere (menu bar,
    /// toolbar) flow into it; fields edited here are kept.
    @State private var base: AppSettings?
    @State private var saveTask: Task<Void, Never>?

    var body: some View {
        Group {
            if draft != nil {
                TabView {
                    GeneralPane(s: binding).tabItem { Label("General", systemImage: "gearshape") }
                    ConnectionPane(s: binding).tabItem { Label("Connection", systemImage: "speedometer") }
                    BrowserPane(s: binding).tabItem { Label("Browser", systemImage: "safari") }
                    CategoriesPane(s: binding).tabItem { Label("File Types", systemImage: "doc.on.doc") }
                    ProxyPane(s: binding).tabItem { Label("Proxy", systemImage: "network") }
                    NotificationsPane(s: binding).tabItem { Label("Notifications", systemImage: "bell") }
                    AdvancedPane(s: binding).tabItem { Label("Advanced", systemImage: "wrench.and.screwdriver") }
                }
                .frame(width: 680, height: 520)
            } else {
                ProgressView().frame(width: 680, height: 520)
            }
        }
        .onAppear {
            draft = store.settings
            base = store.settings
        }
        .onChange(of: store.settings) { s in
            // pick up changes made elsewhere without losing edits in progress
            guard let s = s else { return }
            if let d = draft, let b = base {
                draft = SettingsMerge.merge(base: b, edited: d, latest: s)
            } else {
                draft = s
            }
            base = s
        }
    }

    private var binding: Binding<AppSettings> {
        Binding(get: { draft ?? store.settings! }, set: { v in
            draft = v
            saveTask?.cancel()
            saveTask = Task { @MainActor in
                try? await Task.sleep(nanoseconds: 400_000_000)
                if Task.isCancelled { return }
                // the draft already has newer outside changes merged in
                let d = draft ?? v
                let merged = store.settings.map { SettingsMerge.merge(base: base ?? $0, edited: d, latest: $0) } ?? d
                store.saveSettings(merged)
                // what was stored (e.g. login item adjusted to the real status)
                draft = store.settings ?? merged
                base = store.settings ?? merged
                saveTask = nil
            }
        })
    }
}

/// Three-way merge of settings: start from `latest` and apply every field
/// whose value in `edited` differs from `base` (fields compared as JSON).
enum SettingsMerge {
    static func merge(base: AppSettings, edited: AppSettings, latest: AppSettings) -> AppSettings {
        if edited == base { return latest }
        if latest == base { return edited }
        guard let b = dict(base), let e = dict(edited), var l = dict(latest) else { return edited }
        for k in Set(b.keys).union(e.keys) where !same(b[k], e[k]) {
            l[k] = e[k] // nil removes the key (an optional cleared in the draft)
        }
        guard let data = try? JSONSerialization.data(withJSONObject: l),
              let out = try? JSONDecoder().decode(AppSettings.self, from: data) else { return edited }
        return out
    }

    private static func dict(_ s: AppSettings) -> [String: Any]? {
        guard let d = try? JSONEncoder().encode(s) else { return nil }
        return (try? JSONSerialization.jsonObject(with: d)) as? [String: Any]
    }

    private static func same(_ a: Any?, _ b: Any?) -> Bool {
        switch (a, b) {
        case (nil, nil): return true
        case let (x?, y?): return (x as? NSObject)?.isEqual(y) ?? false
        default: return false
        }
    }
}

// MARK: - panes

private struct GeneralPane: View {
    @Binding var s: AppSettings
    var body: some View {
        Form {
            Toggle("Open at login", isOn: $s.launchAtLogin)
            Toggle("Keep running in the menu bar when the window is closed", isOn: $s.minimizeToTray)
            Toggle("Watch the clipboard for download links", isOn: $s.clipboardMonitor)
                .help("Copying a link to a file type listed under Browser opens the Add dialog.")
            Toggle("Show the progress window when a download starts", isOn: $s.showProgressOnStart)
            Toggle("Show the download window when a download completes", isOn: $s.showCompleteDialog)
            Toggle("Confirm before deleting downloads", isOn: $s.confirmDelete)
            Section("Appearance") {
                Picker("Theme", selection: $s.theme) {
                    Text("Dark").tag("dark")
                    Text("Light").tag("light")
                    Text("Match System").tag("system")
                }
                .pickerStyle(.segmented)
                Toggle("Show download speed in the menu bar", isOn: $s.traySpeed)
            }
        }
        .formStyle(.grouped)
    }
}

private struct ConnectionPane: View {
    @Binding var s: AppSettings
    var body: some View {
        Form {
            Section {
                LabeledContent("Connections per download") {
                    HStack {
                        Slider(value: Binding(get: { Double(s.defaultConnections) }, set: { s.defaultConnections = UInt32($0.rounded()) }), in: 1...32, step: 1)
                            .frame(width: 200)
                        Text("\(s.defaultConnections)").monospacedDigit().frame(width: 28, alignment: .trailing)
                    }
                }
                Text("Files are split dynamically: a connection that finishes takes over half of the largest remaining piece.")
                    .font(.caption).foregroundStyle(.secondary)
                Stepper(value: Binding(get: { Int(s.maxSimultaneous) }, set: { s.maxSimultaneous = UInt32($0) }), in: 1...64) {
                    LabeledContent("Simultaneous downloads", value: "\(s.maxSimultaneous)")
                }
                Text("Outside of queues. Each queue sets its own limit.").font(.caption).foregroundStyle(.secondary)
            }
            Section("Speed limits") {
                MBField(title: "Global limit (all downloads combined)", bps: $s.globalLimitBps, enabled: $s.globalLimitEnabled)
                MBField(title: "Per-download limit", bps: $s.perDownloadLimitBps, enabled: $s.perDownloadLimitEnabled)
                Toggle("Turn on the global limit when the app starts", isOn: $s.limitOnStartup)
            }
        }
        .formStyle(.grouped)
    }
}

private struct MBField: View {
    var title: String
    @Binding var bps: UInt64
    @Binding var enabled: Bool
    @State private var text = ""

    var body: some View {
        LabeledContent(title) {
            HStack {
                TextField("MB/s", text: $text)
                    .frame(width: 70)
                    .multilineTextAlignment(.trailing)
                    .monospacedDigit()
                    .onSubmit(commit)
                    .onChange(of: text) { _ in commit() }
                Text("MB/s").foregroundStyle(.secondary)
                Toggle("", isOn: $enabled).toggleStyle(.switch).labelsHidden()
            }
        }
        .onAppear { text = String(format: "%g", Fmt.mbps(bps)) }
        .onChange(of: bps) { b in
            // changed elsewhere (menu bar, toolbar): show the new value
            if Fmt.limitBps(text) != b { text = String(format: "%g", Fmt.mbps(b)) }
        }
    }

    private func commit() {
        // ignore text that isn't a usable speed (NaN, inf, 0) instead of
        // storing it; the previous value stays
        if let n = Fmt.limitBps(text), n != bps { bps = n }
    }
}

private struct BrowserPane: View {
    @EnvironmentObject var store: AppStore
    @Binding var s: AppSettings
    @State private var extText = ""
    @State private var pairingNote: String?

    private var paused: Bool {
        guard let until = s.capturePausedUntil else { return false }
        return Double(until) / 1000 > Date().timeIntervalSince1970
    }

    var body: some View {
        Form {
            Section {
                LabeledContent("Extension") {
                    HStack(spacing: 6) {
                        Circle().fill(store.ext.bridgeError != nil ? Color.red : store.ext.connected ? Color.green : Color.secondary.opacity(0.5)).frame(width: 8, height: 8)
                        Text(store.ext.bridgeError != nil ? "Not listening" : store.ext.connected ? "Connected: \(store.ext.browsers.joined(separator: ", "))" : "Not detected")
                    }
                }
                if let err = store.ext.bridgeError {
                    Text("\(err). Another app may be using this port; choose a different one under Advanced.")
                        .font(.caption).foregroundStyle(.red)
                }
                Text("Chrome, Edge, Brave, Arc, Vivaldi, Opera: open the extensions page, turn on Developer mode, choose “Load unpacked” and select the chromium folder. Firefox: about:debugging → This Firefox → Load Temporary Add-on → firefox/manifest.json.")
                    .font(.caption).foregroundStyle(.secondary)
                Button("Show Extension Folder") { revealExtension() }
                LabeledContent("Pairing code") {
                    HStack(spacing: 8) {
                        Button("Copy") { copyPairingCode(new: false) }
                        Button("New Code") { copyPairingCode(new: true) }
                        if let note = pairingNote { Text(note).font(.caption).foregroundStyle(.secondary) }
                    }
                }
                Text("Paste it into the extension’s popup, under Connection. Only a paired extension can talk to the app, and other programs can’t read what it sends.")
                    .font(.caption).foregroundStyle(.secondary)
            }
            Section("Capture") {
                Toggle("Take over browser downloads", isOn: $s.captureEnabled)
                Toggle("Pause capture for 10 minutes", isOn: Binding(
                    get: { paused },
                    set: { on in
                        s.capturePausedUntil = on ? Int64((Date().timeIntervalSince1970 + 600) * 1000) : nil
                    }))
                Picker("Bypass key (hold while clicking)", selection: $s.bypassKey) {
                    Text("Option").tag("alt")
                    Text("Shift").tag("shift")
                    Text("Control").tag("ctrl")
                    Text("None").tag("none")
                }
                LabeledContent("Skip files smaller than") {
                    HStack {
                        TextField("KB", value: $s.captureMinSizeKb, format: .number).frame(width: 80).multilineTextAlignment(.trailing)
                        Text("KB").foregroundStyle(.secondary)
                    }
                }
            }
            Section("File types to capture") {
                TextEditor(text: $extText)
                    .font(.system(.body, design: .monospaced))
                    .frame(minHeight: 80)
                    .onChange(of: extText) { t in
                        let list = t.split(whereSeparator: { $0.isWhitespace || $0 == "," || $0 == ";" })
                            .map { $0.trimmingCharacters(in: CharacterSet(charactersIn: ".")).lowercased() }
                            .filter { !$0.isEmpty }
                        if list != s.captureExtensions { s.captureExtensions = list }
                    }
            }
        }
        .formStyle(.grouped)
        .onAppear { extText = s.captureExtensions.joined(separator: " ") }
    }

    /// Put the pairing code (a new one when asked) on the clipboard.
    private func copyPairingCode(new: Bool) {
        do {
            let code: String = try Engine.shared.call(new ? "reset_pairing" : "pairing_code")
            NSPasteboard.general.clearContents()
            NSPasteboard.general.setString(code, forType: .string)
            pairingNote = new ? "New code copied. Pair each browser again." : "Copied."
        } catch {
            store.report(error.localizedDescription)
        }
    }

    private func revealExtension() {
        let candidates = [
            Bundle.main.resourceURL?.appendingPathComponent("extension"),
        ].compactMap { $0 }
        if let dir = candidates.first(where: { FileManager.default.fileExists(atPath: $0.appendingPathComponent("chromium").path) }) {
            NSWorkspace.shared.activateFileViewerSelecting([dir.appendingPathComponent("chromium")])
        } else {
            store.report("The extension isn't bundled in this build. Run extension/build.sh in the source folder and load extension/dist/chromium.")
        }
    }
}

private struct CategoriesPane: View {
    @Binding var s: AppSettings
    @State private var selection: String?
    @State private var newType = ""
    @State private var note = ""

    var body: some View {
        HSplitView {
            List(selection: $selection) {
                ForEach(s.categories) { c in
                    VStack(alignment: .leading, spacing: 2) {
                        Text(c.name)
                        Text(c.id == "general" ? "Everything else" : "\(c.extensions.count) types")
                            .font(.caption).foregroundStyle(.secondary)
                    }
                    .tag(c.id)
                }
            }
            .frame(minWidth: 150, maxWidth: 200)
            .safeAreaInset(edge: .bottom) {
                HStack {
                    Button { addCategory() } label: { Image(systemName: "plus") }
                    Button { removeCategory() } label: { Image(systemName: "minus") }
                        .disabled(selection == nil || selection == "general")
                    Spacer()
                }
                .buttonStyle(.borderless)
                .padding(8)
            }

            if let i = s.categories.firstIndex(where: { $0.id == selection }) {
                editor(i)
            } else {
                Text("Select a category").foregroundStyle(.secondary).frame(maxWidth: .infinity, maxHeight: .infinity)
            }
        }
        .onAppear { if selection == nil { selection = s.categories.first?.id } }
    }

    private func defaultFolder(_ c: Category) -> String {
        if c.id == "general" { return s.defaultFolder }
        return (s.defaultFolder as NSString).appendingPathComponent(c.name.replacingOccurrences(of: "/", with: "_"))
    }

    private func editor(_ i: Int) -> some View {
        let c = $s.categories[i]
        return Form {
            TextField("Name", text: c.name).disabled(c.wrappedValue.id == "general")
            LabeledContent("Folder") {
                HStack {
                    Text(c.wrappedValue.folder ?? defaultFolder(c.wrappedValue))
                        .font(.system(.callout, design: .monospaced))
                        .foregroundStyle(c.wrappedValue.folder == nil ? .secondary : .primary)
                        .lineLimit(1).truncationMode(.middle)
                    Button("Choose…") {
                        if let p = chooseFolder(start: c.wrappedValue.folder ?? defaultFolder(c.wrappedValue)) { c.wrappedValue.folder = p }
                    }
                    if c.wrappedValue.folder != nil {
                        Button("Use Default") { c.wrappedValue.folder = nil }
                    }
                }
            }
            if c.wrappedValue.id == "general" {
                Text("General holds every file type no other category claims, and saves to your download folder.")
                    .foregroundStyle(.secondary)
            } else {
                Section("File types") {
                    FlowTags(tags: c.wrappedValue.extensions) { ext in
                        c.wrappedValue.extensions.removeAll { $0 == ext }
                    }
                    HStack {
                        TextField("Add types (e.g. mkv webm)", text: $newType)
                            .onSubmit { addTypes(to: i) }
                        Button("Add") { addTypes(to: i) }.disabled(newType.trimmingCharacters(in: .whitespaces).isEmpty)
                    }
                    if !note.isEmpty { Text(note).font(.caption).foregroundStyle(.secondary) }
                }
            }
        }
        .formStyle(.grouped)
    }

    private func addTypes(to i: Int) {
        let exts = newType.split(whereSeparator: { $0.isWhitespace || $0 == "," || $0 == ";" })
            .map { $0.trimmingCharacters(in: CharacterSet(charactersIn: ".")).lowercased() }
            .filter { !$0.isEmpty }
        newType = ""
        var cats = s.categories
        var moved: [String] = []
        for e in exts {
            for j in cats.indices where j != i && cats[j].extensions.contains(e) {
                cats[j].extensions.removeAll { $0 == e }
                moved.append(".\(e) moved from \(cats[j].name)")
            }
            if !cats[i].extensions.contains(e) { cats[i].extensions.append(e) }
        }
        s.categories = cats
        note = moved.joined(separator: " · ")
    }

    private func addCategory() {
        let id = "c" + UUID().uuidString.prefix(6).lowercased()
        let c = Category(id: id, name: "New Category", icon: "cat-general", extensions: [], folder: nil)
        var cats = s.categories
        let at = cats.firstIndex(where: { $0.id == "general" }) ?? cats.count
        cats.insert(c, at: at)
        s.categories = cats
        selection = id
    }

    private func removeCategory() {
        guard let sel = selection, sel != "general" else { return }
        s.categories.removeAll { $0.id == sel }
        selection = s.categories.first?.id
    }
}

/// Wrapping row of removable tags.
private struct FlowTags: View {
    var tags: [String]
    var onRemove: (String) -> Void

    var body: some View {
        LazyVGrid(columns: [GridItem(.adaptive(minimum: 74), spacing: 6)], alignment: .leading, spacing: 6) {
            ForEach(tags, id: \.self) { t in
                HStack(spacing: 3) {
                    Text(".\(t)").font(.system(.caption, design: .monospaced))
                    Button { onRemove(t) } label: { Image(systemName: "xmark.circle.fill") }
                        .buttonStyle(.borderless)
                        .foregroundStyle(.secondary)
                        .help("Remove .\(t)")
                }
                .padding(.horizontal, 7)
                .padding(.vertical, 3)
                .background(Capsule().fill(Color.secondary.opacity(0.14)))
            }
        }
    }
}

private struct ProxyPane: View {
    @Binding var s: AppSettings
    var body: some View {
        Form {
            Picker("Connection", selection: $s.proxyMode) {
                Text("No proxy").tag("none")
                Text("Use system settings").tag("system")
                Text("HTTP proxy").tag("http")
                Text("SOCKS5 proxy").tag("socks5")
            }
            if s.proxyMode == "http" || s.proxyMode == "socks5" {
                TextField("Host", text: $s.proxyHost)
                TextField("Port", value: $s.proxyPort, format: .number.grouping(.never))
                TextField("Username", text: $s.proxyUser)
                SecureField("Password", text: $s.proxyPass)
            }
        }
        .formStyle(.grouped)
    }
}

private struct NotificationsPane: View {
    @Binding var s: AppSettings
    var body: some View {
        Form {
            Toggle("Download complete", isOn: $s.notifyComplete)
            Toggle("Download failed", isOn: $s.notifyFailure)
            Toggle("Queue finished", isOn: $s.notifyQueueFinished)
        }
        .formStyle(.grouped)
    }
}

private struct AdvancedPane: View {
    @Binding var s: AppSettings
    var body: some View {
        Form {
            Section("Save locations") {
                LabeledContent("Download folder") {
                    HStack {
                        Text(s.defaultFolder).font(.system(.callout, design: .monospaced)).lineLimit(1).truncationMode(.middle)
                        Button("Choose…") { if let p = chooseFolder(start: s.defaultFolder) { s.defaultFolder = p } }
                    }
                }
                Text("Each category saves to a subfolder of this folder unless you choose another under File Types.")
                    .font(.caption).foregroundStyle(.secondary)
            }
            Section("Network") {
                TextField("User agent", text: $s.userAgent)
                TextField("Retries per connection", value: $s.maxRetries, format: .number)
                TextField("Retry delay (seconds)", value: $s.retryDelaySecs, format: .number)
                TextField("Timeout (seconds)", value: $s.timeoutSecs, format: .number)
                TextField("Minimum segment size (KB)", value: $s.minSegmentKb, format: .number)
                TextField("Extension port", value: $s.extensionPort, format: .number.grouping(.never))
            }
            Section("Security") {
                Toggle("Verify TLS certificates", isOn: $s.verifyTls)
                if !s.verifyTls {
                    Label("Certificate checks are off. Anyone on the network path can alter what you download.", systemImage: "exclamationmark.triangle.fill")
                        .foregroundStyle(.red)
                }
            }
        }
        .formStyle(.grouped)
    }
}
