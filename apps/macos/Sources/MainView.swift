import AppKit
import SwiftUI
import UniformTypeIdentifiers

enum SidebarItem: Hashable {
    case library(String)
    case category(String)
    case queue(String)
}

/// Filtered + sorted row order, kept across renders. Recomputed only when
/// the list's membership/status/names change (AppStore.listVersion), the
/// filter/search/sort change, or — when sorting by a column that moves with
/// progress — on progress ticks. A plain reference, so updating it while
/// rendering doesn't trigger another render.
@MainActor
final class RowCache {
    struct Key: Equatable {
        var version: Int
        var tick: Int
        var filter: SidebarItem?
        var search: String
        var sort: [KeyPathComparator<Download>]
    }
    var key: Key?
    var ids: [String] = []
}

struct MainView: View {
    @EnvironmentObject var store: AppStore
    @State private var filter: SidebarItem? = .library("all")
    @State private var search = ""
    @State private var sortOrder = [KeyPathComparator(\Download.addedAt, order: .reverse)]
    @State private var confirmDelete: [String]?
    @State private var refreshTarget: Download?
    @State private var refreshURL = ""
    @State private var refreshError = ""
    @State private var limitSheet = false
    @State private var limitText = "25"
    @State private var limitError = ""
    @State private var showDetails = true
    @State private var rowCache = RowCache()

    // MARK: data

    private func matches(_ d: Download, _ f: SidebarItem?) -> Bool {
        switch f ?? .library("all") {
        case .library("active"): return d.status.isActive
        case .library("unfinished"): return d.status != .complete
        case .library("finished"): return d.status == .complete
        case .library: return true
        case .category(let c): return d.category == c
        case .queue(let q): return d.queueId == q
        }
    }

    /// Sort columns whose values change with every progress tick.
    private var sortIsLive: Bool {
        let live: [PartialKeyPath<Download>] = [\Download.sizeSort, \Download.progressSort, \Download.speedSort, \Download.etaSort]
        return sortOrder.contains { live.contains($0.keyPath) }
    }

    /// The table rows (computed once per render, order cached; see RowCache).
    private func currentRows() -> [Download] {
        let q = search.trimmingCharacters(in: .whitespaces).lowercased()
        let key = RowCache.Key(version: store.listVersion, tick: sortIsLive ? store.progressVersion : 0,
                               filter: filter, search: q, sort: sortOrder)
        if rowCache.key != key {
            var list = store.downloads.filter { matches($0, filter) }
            if !q.isEmpty {
                list = list.filter {
                    $0.fileName.lowercased().contains(q) || $0.url.lowercased().contains(q) || $0.saveDir.lowercased().contains(q)
                }
            }
            rowCache.ids = list.sorted(using: sortOrder).map(\.id)
            rowCache.key = key
        }
        return rowCache.ids.compactMap { store.download($0) }
    }

    private var selected: [Download] { store.selection.compactMap { store.download($0) } }

    private func focused(_ rows: [Download]) -> Download? {
        if let id = store.selection.first, let d = rows.first(where: { $0.id == id }) { return d }
        return rows.first
    }

    private var currentQueue: String {
        if case .queue(let q) = filter { return q }
        return "main"
    }

    // MARK: body

    var body: some View {
        let rows = currentRows()
        let counts = store.sidebarCounts()
        NavigationSplitView {
            sidebar(counts)
                .navigationSplitViewColumnWidth(min: 180, ideal: 220, max: 300)
        } detail: {
            VStack(spacing: 0) {
                if let p = store.power { powerBanner(p) }
                if let err = store.startError {
                    Text("The download engine failed to start: \(err)")
                        .foregroundStyle(.red).padding(8)
                }
                VSplitView {
                    table(rows)
                        .frame(minHeight: 160)
                    if showDetails, let d = focused(rows) {
                        ScrollView {
                            DetailsPanel(download: d, onRefresh: { startRefresh($0) })
                        }
                        .frame(minHeight: 120, idealHeight: 250, maxHeight: 400)
                    }
                }
                StatusBar()
            }
            .dropDestination(for: URL.self) { urls, _ in
                let web = urls.filter { ["http", "https"].contains($0.scheme?.lowercased() ?? "") }
                if web.count == 1 { store.openAdd(AddParams(url: web[0].absoluteString)) }
                else if web.count > 1 { store.openLinks(LinkParams(links: web.map { .init(url: $0.absoluteString) })) }
                return !web.isEmpty
            }
        }
        .searchable(text: $search, placement: .toolbar, prompt: "Search downloads, URLs, folders")
        .toolbar { toolbar }
        .navigationTitle("Open Download Manager")
        .navigationSubtitle(store.totals.active > 0 ? "\(store.totals.active) active · ↓ \(Fmt.speed(store.totals.speedBps))" : "")
        .confirmationDialog(
            confirmDelete?.count == 1 ? "Delete this download?" : "Delete \(confirmDelete?.count ?? 0) downloads?",
            isPresented: Binding(get: { confirmDelete != nil }, set: { if !$0 { confirmDelete = nil } }),
            titleVisibility: .visible
        ) {
            Button("Remove from List") { store.delete(confirmDelete ?? [], files: false) }
            Button("Remove and Delete Files", role: .destructive) { store.delete(confirmDelete ?? [], files: true) }
            Button("Cancel", role: .cancel) {}
        } message: {
            Text("Unfinished parts are always removed. Finished files stay on disk unless you also delete files.")
        }
        .sheet(item: $refreshTarget) { d in refreshSheet(d) }
        .sheet(isPresented: $limitSheet) { limitEditor }
        .alert("Open Download Manager", isPresented: Binding(get: { store.errorMessage != nil }, set: { if !$0 { store.errorMessage = nil } })) {
            Button("OK", role: .cancel) {}
        } message: {
            Text(store.errorMessage ?? "")
        }
        .onReceive(NotificationCenter.default.publisher(for: .odmAskDelete)) { _ in askDelete(selected.map(\.id)) }
        .onOpenURL { DeepLink.handle($0) }
    }

    // MARK: sidebar

    private func sidebar(_ counts: [SidebarItem: Int]) -> some View {
        List(selection: $filter) {
            Section("Library") {
                sidebarRow("All Downloads", "tray.full", .library("all"), counts)
                sidebarRow("Active", "arrow.down.circle", .library("active"), counts)
                sidebarRow("Unfinished", "circle.lefthalf.filled", .library("unfinished"), counts)
                sidebarRow("Finished", "checkmark.circle", .library("finished"), counts)
            }
            Section("Categories") {
                ForEach(store.settings?.categories ?? []) { c in
                    sidebarRow(c.name, categorySymbol(c.icon), .category(c.id), counts)
                        .contextMenu {
                            SettingsButton(title: "Edit File Types & Folders…")
                        }
                }
            }
            Section("Queues") {
                ForEach(store.queues) { q in
                    sidebarRow(q.name, q.running ? "play.circle.fill" : (q.builtin ? "list.bullet" : "clock"), .queue(q.id), counts)
                        .contextMenu {
                            Button("Start") { store.startQueue(q.id) }.disabled(q.running)
                            Button("Stop") { store.stopQueue(q.id) }.disabled(!q.running)
                            Button("Edit Schedule…") { store.openScheduler() }
                            if !q.builtin {
                                Divider()
                                Button("Delete Queue", role: .destructive) { store.act("delete_queue", ["id": q.id]) }
                            }
                        }
                }
            }
        }
        .listStyle(.sidebar)
    }

    private func sidebarRow(_ title: String, _ symbol: String, _ item: SidebarItem, _ counts: [SidebarItem: Int]) -> some View {
        Label(title, systemImage: symbol)
            .badge(counts[item] ?? 0)
            .tag(item)
    }

    private func categorySymbol(_ icon: String) -> String {
        switch icon {
        case "cat-video": return "film"
        case "cat-music": return "music.note"
        case "cat-documents": return "doc.text"
        case "cat-compressed": return "archivebox"
        case "cat-programs": return "app.badge"
        case "cat-images": return "photo"
        case "file-disc": return "opticaldisc"
        case "cat-grabber": return "globe"
        default: return "doc"
        }
    }

    // MARK: table

    private func table(_ rows: [Download]) -> some View {
        Table(rows, selection: $store.selection, sortOrder: $sortOrder) {
            TableColumn("Name", value: \.fileName) { d in
                HStack(spacing: 8) {
                    FileIconView(fileExtension: d.fileExtension, size: 18)
                    Text(d.fileName).lineLimit(1).truncationMode(.middle)
                }
                .help(d.fileName)
            }
            .width(min: 140, ideal: 320)

            TableColumn("Size", value: \.sizeSort) { d in
                Text(Fmt.bytes(d.totalBytes)).monospacedDigit()
            }
            .width(min: 60, ideal: 86)

            TableColumn("Progress", value: \.progressSort) { d in
                HStack(spacing: 8) {
                    ProgressView(value: Double(Fmt.percent(d)), total: 100)
                        .progressViewStyle(.linear)
                        .tint(Fmt.statusColor(d.status))
                    Text("\(Fmt.percent(d))%").monospacedDigit().frame(width: 38, alignment: .trailing)
                }
            }
            .width(min: 90, ideal: 160)

            TableColumn("Speed", value: \.speedSort) { d in
                Text(d.status.isActive ? Fmt.speed(d.speedBps) : "—")
                    .monospacedDigit()
                    .frame(maxWidth: .infinity, alignment: .trailing)
            }
            .width(min: 60, ideal: 92)

            TableColumn("ETA", value: \.etaSort) { d in
                Text(d.status == .downloading ? Fmt.eta(d.etaSecs) : "—")
                    .monospacedDigit()
                    .frame(maxWidth: .infinity, alignment: .trailing)
            }
            .width(min: 50, ideal: 70)

            TableColumn("Status", value: \.statusSort) { d in
                StatusLabel(download: d)
            }
            .width(min: 80, ideal: 120)

            TableColumn("Queue", value: \.queueSort) { d in
                Text(store.queueName(d.queueId)).foregroundStyle(.secondary)
            }
            .width(min: 50, ideal: 96)

            TableColumn("Added", value: \.addedAt) { d in
                Text(Fmt.added(d.addedAt)).foregroundStyle(.secondary)
            }
            .width(min: 70, ideal: 110)
        }
        .contextMenu(forSelectionType: String.self) { ids in
            rowMenu(Array(ids))
        } primaryAction: { ids in
            guard let id = ids.first, let d = store.download(id) else { return }
            if d.status == .complete { store.openFile(d) } else { store.openProgress(d.id) }
        }
        // the Delete key while the list has focus (text fields keep their own)
        .onDeleteCommand { askDelete(Array(store.selection)) }
        .overlay {
            if store.downloads.isEmpty == false && rows.isEmpty {
                emptyState(search.isEmpty ? "No downloads here" : "No downloads match your search")
            } else if store.downloads.isEmpty {
                emptyState("No downloads yet")
            }
        }
    }

    private func emptyState(_ text: String) -> some View {
        VStack(spacing: 10) {
            Image(systemName: "arrow.down.circle").font(.system(size: 40, weight: .light)).foregroundStyle(.secondary)
            Text(text).foregroundStyle(.secondary)
            Button("Add URL…") { store.openAdd() }
            if store.downloads.isEmpty {
                Text("Or drag a link from your browser onto this window.").font(.caption).foregroundStyle(.secondary)
            }
        }
    }

    @ViewBuilder
    private func rowMenu(_ ids: [String]) -> some View {
        let items = ids.compactMap { store.download($0) }
        if let d = items.first {
            let multi = items.count > 1
            Button("Open") { store.openFile(d) }.disabled(multi || d.status != .complete)
            Button("Show in Finder") { store.revealInFinder(d) }.disabled(multi)
            Button("Show Progress Window") { store.openProgress(d.id) }.disabled(multi || d.status == .complete)
            Divider()
            Button("Resume") { store.resume(ids) }
                .disabled(!items.contains { [.paused, .error, .queued].contains($0.status) })
            Button("Pause") { store.pause(ids) }
                .disabled(!items.contains { $0.status.isActive || $0.status == .queued })
            Button("Redownload") { store.redownload(d.id) }.disabled(multi)
            Button("Refresh Link…") { startRefresh(d) }.disabled(multi || d.status == .complete)
            Button("Copy Link") { store.copyLink(d) }.disabled(multi)
            Divider()
            Menu("Move to Queue") {
                ForEach(store.queues) { q in
                    Button(q.name) { store.moveToQueue(ids, q.id) }
                }
                Divider()
                Button("Remove from Queue") { store.moveToQueue(ids, nil) }
            }
            Menu("Category") {
                ForEach(store.settings?.categories ?? []) { c in
                    Button(c.name) { store.setCategory(ids, c.id) }
                }
            }
            if !multi && !d.fileExtension.isEmpty {
                Menu("Always Put .\(d.fileExtension) Files In") {
                    ForEach(store.settings?.categories ?? []) { c in
                        Button(c.name) { store.assignExtension(d.fileExtension, to: c.id) }
                    }
                }
            }
            Divider()
            Button(multi ? "Delete \(items.count) Downloads…" : "Delete…", role: .destructive) { askDelete(ids) }
        }
    }

    private func askDelete(_ ids: [String]) {
        guard !ids.isEmpty else { return }
        if store.settings?.confirmDelete == false {
            store.delete(ids, files: false)
        } else {
            confirmDelete = ids
        }
    }

    // MARK: toolbar

    @ToolbarContentBuilder
    private var toolbar: some ToolbarContent {
        ToolbarItemGroup(placement: .navigation) {
            Button { store.openAdd() } label: { Label("Add URL", systemImage: "plus") }
                .help("Add URL (⌘N)")
        }
        ToolbarItemGroup(placement: .primaryAction) {
            Button { store.resume(selected.map(\.id)) } label: { Label("Resume", systemImage: "play.fill") }
                .help("Resume (⌘R)")
                .disabled(!selected.contains { [.paused, .error, .queued].contains($0.status) })
            Button { store.pause(selected.map(\.id)) } label: { Label("Pause", systemImage: "pause.fill") }
                .help("Pause (⌘.)")
                .disabled(!selected.contains { $0.status.isActive || $0.status == .queued })
            Button { store.pauseAll() } label: { Label("Pause All", systemImage: "pause.circle") }
                .help("Pause all")
                .disabled(store.totals.active == 0 && store.totals.queued == 0)
            Button { askDelete(selected.map(\.id)) } label: { Label("Delete", systemImage: "trash") }
                .help("Delete (⌘⌫)")
                .disabled(selected.isEmpty)
            Button { store.deleteCompleted() } label: { Label("Delete Completed", systemImage: "checkmark.circle.badge.xmark") }
                .help("Remove completed downloads from the list")
                .disabled(store.totals.complete == 0)
        }
        ToolbarItemGroup(placement: .primaryAction) {
            Menu {
                Button("Start Queue") { store.startQueue(currentQueue) }
                Button("Stop Queue") { store.stopQueue(currentQueue) }
                Divider()
                Button("Scheduler…") { store.openScheduler() }
            } label: {
                Label("Queue", systemImage: "list.bullet.rectangle")
            } primaryAction: {
                if store.queues.first(where: { $0.id == currentQueue })?.running == true {
                    store.stopQueue(currentQueue)
                } else {
                    store.startQueue(currentQueue)
                }
            }
            .help("Start or stop the \(store.queueName(currentQueue)) · hold for more")

            Button { store.openWindowAction?("grabber", nil, nil) } label: { Label("Grabber", systemImage: "globe") }
                .help("Site grabber")

            Menu {
                ForEach([1.0, 2, 5, 10, 25, 50, 100], id: \.self) { mb in
                    Button("\(Int(mb)) MB/s") { store.setGlobalLimit(Fmt.bps(fromMB: mb)) }
                }
                Divider()
                Button("Custom…") {
                    limitText = String(format: "%g", Fmt.mbps(store.settings?.globalLimitBps ?? 0))
                    limitError = ""
                    limitSheet = true
                }
                if store.settings?.globalLimitEnabled == true {
                    Divider()
                    Button("Turn Off Limit") { store.setGlobalLimit(nil) }
                }
            } label: {
                Label(limitLabel, systemImage: store.settings?.globalLimitEnabled == true ? "speedometer" : "gauge.with.dots.needle.67percent")
            } primaryAction: {
                store.toggleLimit()
            }
            .help(store.settings?.globalLimitEnabled == true ? "Speed limit on: \(limitLabel). Click to turn off." : "No speed limit. Click to turn on.")

            Button { showDetails.toggle() } label: { Label("Details", systemImage: "rectangle.bottomthird.inset.filled") }
                .help(showDetails ? "Hide details" : "Show details")
        }
    }

    private var limitLabel: String {
        guard let s = store.settings, s.globalLimitEnabled else { return "No Limit" }
        return String(format: "%g MB/s", Fmt.mbps(s.globalLimitBps))
    }

    private var limitEditor: some View {
        VStack(alignment: .leading, spacing: 14) {
            Text("Global Speed Limit").font(.headline)
            Text("Caps the combined speed of all downloads.").foregroundStyle(.secondary)
            HStack {
                TextField("MB/s", text: $limitText).frame(width: 100).monospacedDigit()
                Text("MB/s").foregroundStyle(.secondary)
            }
            if !limitError.isEmpty {
                Text(limitError).font(.caption).foregroundStyle(.red)
            }
            HStack {
                Spacer()
                Button("Cancel") { limitSheet = false }.keyboardShortcut(.cancelAction)
                Button("Turn On Limit") {
                    // reject NaN / inf / 0 instead of silently doing nothing
                    guard let bps = Fmt.limitBps(limitText) else {
                        limitError = Fmt.limitError
                        return
                    }
                    store.setGlobalLimit(bps)
                    limitSheet = false
                }
                .keyboardShortcut(.defaultAction)
            }
        }
        .padding(20)
        .frame(width: 320)
    }

    // MARK: refresh link

    private func startRefresh(_ d: Download) {
        refreshURL = d.url
        refreshError = ""
        refreshTarget = d
    }

    private func refreshSheet(_ d: Download) -> some View {
        VStack(alignment: .leading, spacing: 12) {
            Text("Refresh Download Link").font(.headline)
            Text("Paste a fresh address for the same file. Progress is kept when the server reports the same size.")
                .foregroundStyle(.secondary)
                .fixedSize(horizontal: false, vertical: true)
            TextField("https://", text: $refreshURL)
                .font(.system(.body, design: .monospaced))
                .textFieldStyle(.roundedBorder)
            if !refreshError.isEmpty { Text(refreshError).foregroundStyle(.red).textSelection(.enabled) }
            HStack {
                Spacer()
                Button("Cancel") { refreshTarget = nil }.keyboardShortcut(.cancelAction)
                Button("Update Link") {
                    let id = d.id, url = refreshURL.trimmingCharacters(in: .whitespaces)
                    Task {
                        do {
                            try await Engine.shared.runAwait("refresh_link", ["id": id, "newUrl": url])
                            refreshTarget = nil
                        } catch {
                            refreshError = error.localizedDescription
                        }
                    }
                }
                .keyboardShortcut(.defaultAction)
            }
        }
        .padding(20)
        .frame(width: 520)
    }

    // MARK: power banner

    private func powerBanner(_ p: AppStore.PowerCountdown) -> some View {
        TimelineView(.periodic(from: .now, by: 1)) { ctx in
            let left = max(0, Int(p.deadline.timeIntervalSince(ctx.date)))
            let verb = p.action == "sleep" ? "Sleeping" : p.action == "shutdown" ? "Shutting down" : "Quitting"
            HStack {
                Image(systemName: p.action == "sleep" ? "moon.zzz" : "power")
                Text("“\(p.queue)” finished. \(verb) in \(left)s.")
                Spacer()
                Button("Cancel") { store.cancelPower() }
            }
            .padding(.horizontal, 14)
            .padding(.vertical, 8)
            .background(Color.orange.opacity(0.18))
        }
    }
}

// MARK: - Details panel

struct DetailsPanel: View {
    @EnvironmentObject var store: AppStore
    var download: Download
    var onRefresh: (Download) -> Void

    var body: some View {
        let d = download
        VStack(alignment: .leading, spacing: 12) {
            HStack(spacing: 12) {
                FileIconView(fileExtension: d.fileExtension, size: 36)
                VStack(alignment: .leading, spacing: 2) {
                    Text(d.fileName).font(.headline).lineLimit(1).truncationMode(.middle).textSelection(.enabled)
                    Text(d.url).font(.system(.caption, design: .monospaced)).foregroundStyle(.secondary)
                        .lineLimit(1).truncationMode(.middle).textSelection(.enabled)
                }
                Spacer()
                StatusLabel(download: d)
                primaryButton(d)
                Button { store.revealInFinder(d) } label: { Label("Show in Finder", systemImage: "folder") }
                Button { store.copyLink(d) } label: { Label("Copy Link", systemImage: "link") }
            }
            if d.status == .error, let e = d.error {
                Text(e)
                    .font(.system(.callout, design: .monospaced))
                    .foregroundStyle(.red)
                    .textSelection(.enabled)
                    .padding(8)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .background(RoundedRectangle(cornerRadius: 6).fill(Color.red.opacity(0.1)))
            }
            HStack(alignment: .top, spacing: 24) {
                Grid(alignment: .leading, horizontalSpacing: 14, verticalSpacing: 6) {
                    InfoRow(label: "Downloaded", value: "\(Fmt.bytes(d.receivedBytes)) of \(Fmt.bytes(d.totalBytes))")
                    InfoRow(label: "Speed", value: d.status.isActive ? Fmt.speed(d.speedBps) : "—")
                    InfoRow(label: "Time left", value: d.status == .downloading ? Fmt.eta(d.etaSecs) : "—")
                    InfoRow(label: "Connections", value: "\(d.activeConnections) active / \(d.maxConnections) max")
                    InfoRow(label: "Resume", value: d.resumable ? "Supported (byte ranges)" : "Not supported by server", mono: false)
                    InfoRow(label: "Save to", value: d.saveDir)
                }
                .font(.callout)
                .frame(minWidth: 280, maxWidth: 420, alignment: .leading)
                VStack(alignment: .leading, spacing: 10) {
                    HStack {
                        Text("Segments").foregroundStyle(.secondary)
                        Spacer()
                        Text("\(Fmt.percent(d))%").monospacedDigit().foregroundStyle(.secondary)
                    }
                    .font(.caption)
                    SegmentMapView(segments: d.segments, status: d.status, total: d.totalBytes, height: 18)
                    SparklineView(samples: d.speedHistory, height: 52)
                }
            }
        }
        .padding(16)
        .frame(maxWidth: .infinity, alignment: .topLeading)
        .background(.background)
    }

    @ViewBuilder
    private func primaryButton(_ d: Download) -> some View {
        switch d.status {
        case .downloading, .connecting, .assembling, .queued:
            Button { store.pause([d.id]) } label: { Label("Pause", systemImage: "pause.fill") }
        case .complete:
            Button { store.openFile(d) } label: { Label("Open", systemImage: "arrow.up.forward.app") }
        case .error:
            Button { onRefresh(d) } label: { Label("Refresh Link…", systemImage: "arrow.clockwise") }
            Button { store.resume([d.id]) } label: { Label("Retry", systemImage: "play.fill") }
        case .paused:
            Button { store.resume([d.id]) } label: { Label("Resume", systemImage: "play.fill") }
        }
    }
}

// MARK: - Status bar

struct StatusBar: View {
    @EnvironmentObject var store: AppStore

    var body: some View {
        let t = store.totals
        HStack(spacing: 18) {
            Text(summary(t))
            Text("↓ \(Fmt.speed(t.speedBps))").monospacedDigit()
            Text(t.limitOn ? String(format: "Limit %g MB/s", Fmt.mbps(t.limitBps)) : "No speed limit")
            HStack(spacing: 5) {
                Circle().fill(store.ext.bridgeError != nil ? Color.red : store.ext.connected ? Color.green : Color.secondary.opacity(0.5)).frame(width: 7, height: 7)
                Text(store.ext.bridgeError != nil
                     ? "Browser bridge offline: port in use"
                     : store.ext.connected
                     ? "Browser extension connected\(store.ext.browsers.isEmpty ? "" : " (\(store.ext.browsers.joined(separator: ", ")))")"
                     : "Browser extension not detected")
            }
            .help(store.ext.bridgeError ?? "")
            Spacer()
            if let f = t.freeBytes { Text("\(Fmt.bytes(f)) free").monospacedDigit() }
        }
        .font(.caption)
        .foregroundStyle(.secondary)
        .padding(.horizontal, 14)
        .padding(.vertical, 6)
        .background(.bar)
    }

    private func summary(_ t: Totals) -> String {
        var parts = ["\(t.active) active"]
        if t.paused > 0 { parts.append("\(t.paused) paused") }
        if t.queued > 0 { parts.append("\(t.queued) queued") }
        if t.scheduled > 0 { parts.append("\(t.scheduled) scheduled") }
        if t.failed > 0 { parts.append("\(t.failed) failed") }
        return parts.joined(separator: " · ")
    }
}
