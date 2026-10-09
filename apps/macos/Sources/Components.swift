import AppKit
import SwiftUI
import UniformTypeIdentifiers

/// The file-type icon macOS uses in Finder.
enum FileIcons {
    private static var cache: [String: NSImage] = [:]

    static func icon(forExtension ext: String) -> NSImage {
        let key = ext.lowercased()
        if let i = cache[key] { return i }
        let type = UTType(filenameExtension: key) ?? .data
        let img = NSWorkspace.shared.icon(for: type)
        cache[key] = img
        return img
    }
}

struct FileIconView: View {
    var fileExtension: String
    var size: CGFloat = 18

    var body: some View {
        Image(nsImage: FileIcons.icon(forExtension: fileExtension))
            .resizable()
            .interpolation(.high)
            .frame(width: size, height: size)
    }
}

struct StatusLabel: View {
    var download: Download

    var body: some View {
        HStack(spacing: 6) {
            Circle()
                .fill(Fmt.statusColor(download.status))
                .frame(width: 7, height: 7)
            Text(Fmt.statusLabel(download))
        }
        .accessibilityElement(children: .combine)
    }
}

/// One cell per connection segment, sized by its share of the file.
struct SegmentMapView: View {
    var segments: [Segment]
    var status: DownloadStatus
    var total: UInt64?
    var height: CGFloat = 18
    var showWriteHead = false

    var body: some View {
        Canvas { ctx, size in
            let segs = segments.sorted { $0.start < $1.start }
            let color = Fmt.statusColor(status)
            let track = Color.secondary.opacity(0.18)
            let gap: CGFloat = 3
            if segs.isEmpty {
                let r = CGRect(origin: .zero, size: size)
                ctx.fill(Path(roundedRect: r, cornerRadius: 4), with: .color(track))
                if status == .complete {
                    ctx.fill(Path(roundedRect: r, cornerRadius: 4), with: .color(color.opacity(0.55)))
                }
                return
            }
            let tot = Double(total ?? segs.map { $0.start + $0.received }.max() ?? 1)
            let sizes: [Double] = segs.map { s in
                let len = s.knownEnd ? Double(s.end - s.start) : max(1, tot - Double(s.start))
                return max(len / max(tot, 1), 0.012)
            }
            let sum = sizes.reduce(0, +)
            let usable = size.width - gap * CGFloat(segs.count - 1)
            var x: CGFloat = 0
            for (i, s) in segs.enumerated() {
                let w = max(3, usable * CGFloat(sizes[i] / sum))
                let rect = CGRect(x: x, y: 0, width: w, height: size.height)
                ctx.fill(Path(roundedRect: rect, cornerRadius: 3), with: .color(track))
                let len = s.knownEnd ? Double(s.end - s.start) : max(1, tot - Double(s.start))
                let frac = status == .complete ? 1.0 : min(1.0, Double(s.received) / max(len, 1))
                if frac > 0 {
                    let done = s.state == .done || frac >= 1
                    let fr = CGRect(x: x, y: 0, width: w * CGFloat(frac), height: size.height)
                    ctx.fill(Path(roundedRect: fr, cornerRadius: 3), with: .color(color.opacity(done ? 0.55 : 1)))
                    if showWriteHead && s.state == .receiving && frac < 1 {
                        let hw = min(8, fr.width)
                        let head = CGRect(x: fr.maxX - hw, y: 0, width: hw, height: size.height)
                        ctx.fill(Path(head), with: .color(.white.opacity(0.35)))
                    }
                }
                x += w + gap
            }
        }
        .frame(height: height)
        .accessibilityLabel("Segment map, \(segments.count) segments")
    }
}

/// 60-second speed history.
struct SparklineView: View {
    var samples: [UInt64]
    var height: CGFloat = 56

    var body: some View {
        VStack(alignment: .leading, spacing: 4) {
            HStack {
                Text("Speed, last 60 s").foregroundStyle(.secondary)
                Spacer()
                Text("peak \(Fmt.speed(samples.max()))").monospacedDigit().foregroundStyle(.secondary)
            }
            .font(.caption)
            Canvas { ctx, size in
                let s = Array(samples.suffix(60))
                guard s.count > 1 else { return }
                let peak = Double(max(1, s.max() ?? 1))
                let off = 60 - s.count
                func pt(_ i: Int) -> CGPoint {
                    CGPoint(x: size.width * CGFloat(off + i) / 59,
                            y: size.height - 3 - CGFloat(Double(s[i]) / peak) * (size.height - 8))
                }
                var line = Path()
                line.move(to: pt(0))
                for i in 1..<s.count { line.addLine(to: pt(i)) }
                var area = line
                area.addLine(to: CGPoint(x: pt(s.count - 1).x, y: size.height))
                area.addLine(to: CGPoint(x: pt(0).x, y: size.height))
                area.closeSubpath()
                ctx.fill(area, with: .color(.accentColor.opacity(0.18)))
                ctx.stroke(line, with: .color(.accentColor), lineWidth: 1.6)
            }
            .frame(height: height)
            .background(RoundedRectangle(cornerRadius: 6).fill(Color.secondary.opacity(0.08)))
        }
    }
}

/// Labeled value used in detail grids.
struct InfoRow: View {
    var label: String
    var value: String
    var mono = true

    var body: some View {
        GridRow {
            Text(label).foregroundStyle(.secondary).gridColumnAlignment(.leading)
            Text(value)
                .font(mono ? .system(.body, design: .monospaced) : .body)
                .lineLimit(1)
                .truncationMode(.middle)
                .textSelection(.enabled)
        }
    }
}

/// Close the window that hosts the calling view.
@MainActor
func closeKeyWindow() {
    NSApp.keyWindow?.performClose(nil)
}

@MainActor
func chooseFolder(start: String?) -> String? {
    let p = NSOpenPanel()
    p.canChooseDirectories = true
    p.canChooseFiles = false
    p.canCreateDirectories = true
    p.allowsMultipleSelection = false
    if let s = start, !s.isEmpty { p.directoryURL = URL(fileURLWithPath: s, isDirectory: true) }
    return p.runModal() == .OK ? p.url?.path : nil
}
