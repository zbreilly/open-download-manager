import SwiftUI

struct GrabberView: View {
    @EnvironmentObject var store: AppStore

    struct Row: Identifiable, Hashable {
        var url: String
        var fileName: String
        var ext: String
        var on: Bool
        /// This link's own cookies (never shared with other links).
        var cookies: String? = nil
        var id: String { url }
    }

    /// Keep the first row for each URL: rows are identified by URL.
    private static func dedupe(_ rows: [Row]) -> [Row] {
        var seen = Set<String>()
        return rows.filter { seen.insert($0.url).inserted }
    }

    @State private var pageURL = ""
    @State private var depth = 0
    @State private var preset = "all"
    @State private var custom = ""
    @State private var sameHost = true
    @State private var busy = false
    @State private var error = ""
    @State private var rows: [Row] = []
    @State private var filterText = ""
    @State private var saveDir = ""
    @State private var queueId = ""
    @State private var fromList = false
    @State private var referrer: String?
    @State private var userAgent: String?
    @State private var host = HostWindow()

    private static let presets: [(String, String, String)] = [
        ("all", "All files", ""),
        ("images", "Images", "jpg jpeg png gif webp bmp svg tif tiff avif heic"),
        ("video", "Video", "mp4 mkv avi mov wmv flv webm m4v mpg mpeg ts"),
        ("audio", "Audio", "mp3 wav flac aac m4a ogg opus wma"),
        ("documents", "Documents", "pdf doc docx xls xlsx ppt pptx odt ods txt epub csv"),
        ("archives", "Archives", "zip rar 7z tar gz tgz bz2 xz zst"),
        ("programs", "Programs", "exe msi dmg pkg deb rpm appimage apk iso"),
        ("custom", "Custom…", ""),
    ]

    private var exts: [String] {
        let src = preset == "custom" ? custom : (Self.presets.first { $0.0 == preset }?.2 ?? "")
        return src.split(whereSeparator: { $0.isWhitespace || $0 == "," }).map(String.init)
    }
    private var visible: [Row] {
        filterText.isEmpty ? rows : rows.filter { $0.url.localizedCaseInsensitiveContains(filterText) }
    }
    private var chosen: [Row] { rows.filter(\.on) }

    var body: some View {
        VStack(spacing: 0) {
            if !fromList {
                HStack(alignment: .firstTextBaseline) {
                    TextField("Page address", text: $pageURL, prompt: Text("https://example.com/gallery/"))
                        .textFieldStyle(.roundedBorder)
                        .font(.system(.body, design: .monospaced))
                        .onSubmit(run)
                    Picker("Depth", selection: $depth) {
                        Text("This page").tag(0)
                        Text("+1 level").tag(1)
                        Text("+2 levels").tag(2)
                    }
                    .fixedSize()
                    Picker("Types", selection: $preset) {
                        ForEach(Self.presets, id: \.0) { p in Text(p.1).tag(p.0) }
                    }
                    .fixedSize()
                    Button(busy ? "Scanning…" : "Find Files", action: run)
                        .keyboardShortcut(.defaultAction)
                        .disabled(busy || pageURL.trimmingCharacters(in: .whitespaces).isEmpty)
                }
                .padding([.horizontal, .top], 14)
                HStack {
                    if preset == "custom" {
                        TextField("Extensions", text: $custom, prompt: Text("jpg png zip")).textFieldStyle(.roundedBorder).frame(width: 240)
                    }
                    if depth > 0 { Toggle("Stay on the same site", isOn: $sameHost) }
                    Spacer()
                }
                .padding(.horizontal, 14)
                .padding(.top, 6)
            }
            if !error.isEmpty {
                Text(error).foregroundStyle(.red).textSelection(.enabled).padding(.horizontal, 14).padding(.top, 6)
                    .frame(maxWidth: .infinity, alignment: .leading)
            }
            HStack {
                Text("\(chosen.count) of \(rows.count) selected").foregroundStyle(.secondary)
                Button("Select All") { for i in rows.indices { rows[i].on = true } }.buttonStyle(.link)
                Button("None") { for i in rows.indices { rows[i].on = false } }.buttonStyle(.link)
                Spacer()
                TextField("Filter", text: $filterText).textFieldStyle(.roundedBorder).frame(width: 200)
            }
            .padding(14)
            Table(visible) {
                TableColumn("") { r in
                    Toggle("", isOn: Binding(
                        get: { r.on },
                        set: { v in if let i = rows.firstIndex(where: { $0.id == r.id }) { rows[i].on = v } }))
                    .labelsHidden()
                }
                .width(24)
                TableColumn("Name") { r in
                    HStack {
                        FileIconView(fileExtension: r.ext.lowercased(), size: 16)
                        Text(r.fileName).lineLimit(1)
                    }
                }
                .width(min: 120, ideal: 240)
                TableColumn("Address") { r in
                    Text(r.url).font(.system(.caption, design: .monospaced)).foregroundStyle(.secondary)
                        .lineLimit(1).truncationMode(.middle)
                }
            }
            .overlay {
                if rows.isEmpty {
                    Text(busy ? "Scanning…" : "Files found on the page appear here.").foregroundStyle(.secondary)
                }
            }
            Divider()
            HStack {
                Text("Save to").foregroundStyle(.secondary)
                Text(saveDir.isEmpty ? "Category folders" : saveDir)
                    .font(.system(.callout, design: .monospaced)).lineLimit(1).truncationMode(.middle)
                Button("Choose…") { if let p = chooseFolder(start: saveDir) { saveDir = p } }
                if !saveDir.isEmpty { Button("Use Category Folders") { saveDir = "" } }
                Spacer()
                Picker("Queue", selection: $queueId) {
                    Text("None (start now)").tag("")
                    ForEach(store.queues) { q in Text(q.name).tag(q.id) }
                }
                .fixedSize()
            }
            .padding(.horizontal, 14)
            .padding(.top, 10)
            HStack {
                Button("Add to Queue") { add(start: false) }.disabled(chosen.isEmpty || busy)
                Spacer()
                Button("Cancel") { host.close() }.keyboardShortcut(.cancelAction)
                Button("Download \(chosen.count)") { add(start: true) }.disabled(chosen.isEmpty || busy)
            }
            .padding(14)
        }
        .onAppear(perform: takePending)
        .onChange(of: store.pendingLinks) { _ in takePending() }
        .background(WindowAccessor(host: host))
    }

    private func takePending() {
        guard let p = store.pendingLinks else { return }
        store.pendingLinks = nil
        fromList = true
        error = ""
        // a new list is a new context: nothing carries over from the last one
        referrer = p.referrer.flatMap { $0.isEmpty ? nil : $0 }
        userAgent = p.userAgent.flatMap { $0.isEmpty ? nil : $0 }
        rows = Self.dedupe(p.links.filter { !$0.url.isEmpty }.map { l in
            let name = URL(string: l.url).map { $0.lastPathComponent.removingPercentEncoding ?? $0.lastPathComponent } ?? l.url
            let ext = (name as NSString).pathExtension
            let c = l.cookies.flatMap { $0.isEmpty ? nil : $0 }
            return Row(url: l.url, fileName: name.isEmpty ? l.url : name, ext: ext.isEmpty ? "FILE" : ext.uppercased(), on: true, cookies: c)
        })
    }

    private func run() {
        let u = pageURL.trimmingCharacters(in: .whitespaces)
        guard !u.isEmpty else { return }
        busy = true
        error = ""
        fromList = false
        let e = exts, d = depth, same = sameHost
        Task {
            do {
                let found: [GrabbedLink] = try await Engine.shared.callAsync("grab", ["url": u, "depth": d, "extensions": e, "sameHost": same])
                rows = Self.dedupe(found.map { Row(url: $0.url, fileName: $0.fileName, ext: $0.ext, on: true) })
                referrer = u
                userAgent = nil
                if found.isEmpty { error = "No matching files were found on that page." }
            } catch {
                self.error = error.localizedDescription
            }
            busy = false
        }
    }

    private func add(start: Bool) {
        let e = Engine.shared
        let reqs = chosen.map { r in
            AddRequest(url: r.url, saveDir: saveDir.isEmpty ? nil : saveDir,
                       queueId: queueId.isEmpty ? (start ? nil : "main") : queueId,
                       start: start, connections: store.settings?.defaultConnections,
                       referrer: referrer, cookies: r.cookies, userAgent: userAgent)
        }
        busy = true
        Task {
            do {
                let _: [String] = try await e.callAsync("add_many", ["reqs": e.jsonValue(reqs)])
                host.close()
            } catch {
                self.error = error.localizedDescription
            }
            busy = false
        }
    }
}
