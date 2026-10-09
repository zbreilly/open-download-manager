import AppKit
import SwiftUI

/// Per-download progress window (IDM's download dialog).
struct DownloadWindowView: View {
    @EnvironmentObject var store: AppStore
    var id: String

    @State private var limitOn = false
    @State private var limitMB = "5"
    @State private var confirmCancel = false
    @State private var sawIncomplete = false
    @State private var closeAt: Date?
    @State private var hovering = false
    @State private var initialized = false

    var body: some View {
        Group {
            if let d = store.download(id) {
                content(d)
            } else {
                Text(store.downloads.isEmpty ? "Loading…" : "This download no longer exists.")
                    .foregroundStyle(.secondary)
                    .frame(maxWidth: .infinity, maxHeight: .infinity)
            }
        }
        .frame(minWidth: 620, minHeight: 520)
        .onHover { hovering = $0 }
    }

    private func content(_ d: Download) -> some View {
        VStack(spacing: 0) {
            VStack(alignment: .leading, spacing: 16) {
                header(d)
                hero(d)
                if d.status == .error, let e = d.error {
                    Text(e).font(.system(.callout, design: .monospaced)).foregroundStyle(.red).textSelection(.enabled)
                }
                if d.checksumOk == true {
                    Label("Checksum verified", systemImage: "checkmark.seal").foregroundStyle(.green)
                }
                VStack(alignment: .leading, spacing: 6) {
                    HStack {
                        Text("Segment map · \(max(Int(d.activeConnections), d.segments.count)) connections")
                        Spacer()
                        Text("Faded = segment done · light edge = write position")
                    }
                    .font(.caption).foregroundStyle(.secondary)
                    SegmentMapView(segments: d.segments, status: d.status, total: d.totalBytes, height: 26, showWriteHead: true)
                }
                HStack(alignment: .top, spacing: 16) {
                    connectionTable(d)
                    VStack(alignment: .leading, spacing: 12) {
                        SparklineView(samples: d.speedHistory, height: 80)
                        GroupBox {
                            VStack(alignment: .leading, spacing: 8) {
                                Toggle("Limit this download", isOn: $limitOn)
                                    .onChange(of: limitOn) { _ in applyLimit() }
                                if limitOn {
                                    HStack {
                                        TextField("MB/s", text: $limitMB).frame(width: 80).monospacedDigit()
                                            .onSubmit { applyLimit() }
                                        Text("MB/s").foregroundStyle(.secondary)
                                        Button("Apply") { applyLimit() }
                                    }
                                }
                                Text("Overrides the per-download setting.").font(.caption).foregroundStyle(.secondary)
                            }
                            .frame(maxWidth: .infinity, alignment: .leading)
                        }
                        Picker("When finished", selection: Binding(
                            get: { d.whenFinished },
                            set: { v in store.act("update_download", ["id": d.id, "patch": ["whenFinished": v]]) })) {
                            Text("Show notification").tag("notify")
                            Text("Open file").tag("openFile")
                            Text("Show in Finder").tag("openFolder")
                            Text("Do nothing").tag("none")
                        }
                    }
                    .frame(maxWidth: 300)
                }
            }
            .padding(20)
            Spacer(minLength: 0)
            Divider()
            footer(d)
        }
        .onAppear { setup(d) }
        .onChange(of: d.status) { s in statusChanged(s) }
        .confirmationDialog("Cancel this download?", isPresented: $confirmCancel, titleVisibility: .visible) {
            Button("Cancel Download", role: .destructive) {
                store.delete([d.id], files: false)
                closeKeyWindow()
            }
            Button("Keep Downloading", role: .cancel) {}
        } message: {
            Text("The partial file is deleted and the download is removed from the list.")
        }
        .navigationTitle("\(Fmt.percent(d))% \(d.fileName)")
    }

    private func header(_ d: Download) -> some View {
        HStack(spacing: 12) {
            FileIconView(fileExtension: d.fileExtension, size: 40)
            VStack(alignment: .leading, spacing: 2) {
                Text(d.fileName).font(.title3.weight(.semibold)).lineLimit(1).truncationMode(.middle).textSelection(.enabled)
                Text("\(Fmt.host(d.finalUrl ?? d.url)) · \(d.saveDir)")
                    .font(.system(.caption, design: .monospaced)).foregroundStyle(.secondary)
                    .lineLimit(1).truncationMode(.middle)
            }
            Spacer()
            StatusLabel(download: d)
        }
    }

    private func hero(_ d: Download) -> some View {
        HStack(alignment: .lastTextBaseline, spacing: 24) {
            if d.status == .complete {
                Label("Complete", systemImage: "checkmark.circle.fill")
                    .font(.system(size: 28, weight: .semibold))
                    .foregroundStyle(.green)
            } else {
                HStack(alignment: .lastTextBaseline, spacing: 2) {
                    Text("\(Fmt.percent(d))").font(.system(size: 44, weight: .medium, design: .rounded)).monospacedDigit()
                    Text("%").font(.title2).foregroundStyle(.secondary)
                }
            }
            Grid(alignment: .leading, horizontalSpacing: 24, verticalSpacing: 2) {
                GridRow {
                    stat("Downloaded")
                    stat("Speed")
                    stat("Time left")
                    stat("Resume")
                }
                GridRow {
                    Text("\(Fmt.bytes(d.receivedBytes)) / \(Fmt.bytes(d.totalBytes))").monospacedDigit()
                    Text(d.status.isActive ? Fmt.speed(d.speedBps) : "—").monospacedDigit().foregroundStyle(Color.accentColor)
                    Text(d.status == .downloading ? Fmt.eta(d.etaSecs) : "—").monospacedDigit()
                    Text(d.resumable ? "Supported" : "Not supported")
                }
            }
        }
    }

    private func stat(_ s: String) -> some View {
        Text(s.uppercased()).font(.caption2.weight(.semibold)).foregroundStyle(.secondary)
    }

    private struct ConnRow: Identifiable {
        let id: Int
        let received: UInt64
        let size: UInt64?
        let state: SegState
        let speed: UInt64
    }

    private func connectionTable(_ d: Download) -> some View {
        let rows: [ConnRow] = d.segments.sorted { $0.start < $1.start }.enumerated().map { i, s in
            ConnRow(id: i + 1, received: s.received, size: s.knownEnd ? s.end - s.start : nil, state: s.state, speed: s.speedBps ?? 0)
        }
        return Table(rows) {
            TableColumn("#") { r in Text(String(format: "%02d", r.id)).monospacedDigit().foregroundStyle(.secondary) }
                .width(30)
            TableColumn("Received") { r in
                Text("\(Fmt.bytes(r.received))\(r.size.map { " / \(Fmt.bytes($0))" } ?? "")").monospacedDigit()
            }
            TableColumn("State") { r in
                Text(stateLabel(r.state)).foregroundStyle(stateColor(r.state))
            }
            .width(min: 70, ideal: 90)
            TableColumn("Speed") { r in
                Text(r.state == .receiving ? Fmt.speed(r.speed) : "—").monospacedDigit()
                    .frame(maxWidth: .infinity, alignment: .trailing)
            }
            .width(min: 60, ideal: 84)
        }
        .frame(minHeight: 160)
    }

    private func stateLabel(_ s: SegState) -> String {
        switch s {
        case .receiving: return "Receiving"
        case .reconnecting: return "Reconnecting"
        case .done: return "Done"
        case .connecting: return "Connecting"
        case .idle: return "Idle"
        }
    }

    private func stateColor(_ s: SegState) -> Color {
        switch s {
        case .receiving: return .primary
        case .reconnecting: return .orange
        case .done: return .green
        default: return .secondary
        }
    }

    private func footer(_ d: Download) -> some View {
        HStack {
            if d.status == .complete {
                Button(closeAt.map { _ in "Close" } ?? "Close") { closeKeyWindow() }
                if closeAt != nil {
                    TimelineView(.periodic(from: .now, by: 1)) { ctx in
                        let left = max(0, Int((closeAt ?? ctx.date).timeIntervalSince(ctx.date)))
                        Text("Closing in \(left)s").font(.caption).foregroundStyle(.secondary)
                            .onChange(of: left) { l in
                                if l == 0 && !hovering { closeKeyWindow() }
                                else if hovering { closeAt = nil }
                            }
                    }
                }
                Spacer()
                Button { store.revealInFinder(d) } label: { Label("Show in Finder", systemImage: "folder") }
                Button { store.openFile(d); closeKeyWindow() } label: { Label("Open", systemImage: "arrow.up.forward.app") }
                    .keyboardShortcut(.defaultAction)
            } else {
                Button("Hide") { closeKeyWindow() }.keyboardShortcut(.cancelAction)
                Spacer()
                Button("Cancel Download", role: .destructive) { confirmCancel = true }
                if d.status.isActive || d.status == .queued {
                    Button { store.pause([d.id]) } label: { Label("Pause", systemImage: "pause.fill") }
                        .keyboardShortcut(.defaultAction)
                } else {
                    Button { store.resume([d.id]) } label: { Label(d.status == .error ? "Retry" : "Resume", systemImage: "play.fill") }
                        .keyboardShortcut(.defaultAction)
                }
            }
        }
        .padding(14)
    }

    private func setup(_ d: Download) {
        guard !initialized else { return }
        initialized = true
        sawIncomplete = d.status != .complete
        if let l = d.speedLimitBps, l > 0 {
            limitOn = true
            limitMB = String(format: "%g", Fmt.mbps(l))
        }
    }

    private func statusChanged(_ s: DownloadStatus) {
        if s != .complete { sawIncomplete = true; closeAt = nil }
        if s == .complete && sawIncomplete { closeAt = Date().addingTimeInterval(5) }
    }

    private func applyLimit() {
        let v = Double(limitMB.replacingOccurrences(of: ",", with: ".")) ?? 0
        let bps: UInt64? = (limitOn && v > 0) ? Fmt.bps(fromMB: v) : nil
        store.act("set_download_limit", ["id": id, "bps": orNull(bps)])
    }
}
