import QtQuick
import org.kde.kirigami as Kirigami

// One cell per connection segment, sized by its share of the file.
Canvas {
    id: map

    // the download as the engine reports it
    property var download: null
    // mark where each receiving segment is writing
    property bool showWriteHead: false
    property color trackColor: Qt.alpha(Kirigami.Theme.textColor, 0.15)

    implicitHeight: 18
    Accessible.role: Accessible.Graphic
    Accessible.name: "Segment map, " + (download && download.segments ? download.segments.length : 0) + " segments"

    onDownloadChanged: requestPaint()
    onWidthChanged: requestPaint()
    onTrackColorChanged: requestPaint()

    function statusColor(status) {
        switch (status) {
        case "downloading":
        case "connecting":
        case "assembling": return Kirigami.Theme.highlightColor
        case "paused": return Kirigami.Theme.neutralTextColor
        case "error": return Kirigami.Theme.negativeTextColor
        case "complete": return Kirigami.Theme.positiveTextColor
        }
        return Kirigami.Theme.disabledTextColor
    }

    function box(ctx, x, w, color, alpha, radius) {
        if (w <= 0)
            return
        ctx.globalAlpha = alpha
        ctx.fillStyle = color
        ctx.beginPath()
        ctx.roundedRect(x, 0, w, height, radius, radius)
        ctx.fill()
        ctx.globalAlpha = 1
    }

    onPaint: {
        const ctx = getContext("2d")
        ctx.reset()
        if (!download || width <= 0)
            return
        const status = download.status
        const color = statusColor(status)
        const segs = (download.segments || []).slice().sort((a, b) => a.start - b.start)
        if (segs.length === 0) {
            box(ctx, 0, width, trackColor, 1, 4)
            if (status === "complete")
                box(ctx, 0, width, color, 0.55, 4)
            return
        }
        // an end near 2^64 means "until the end of the file"
        const known = s => s.end < 9e18
        let total = download.totalBytes
        if (!total)
            total = Math.max(1, ...segs.map(s => s.start + s.received))
        const len = s => known(s) ? Math.max(0, s.end - s.start) : Math.max(1, total - s.start)
        const sizes = segs.map(s => Math.max(len(s) / total, 0.012))
        const sum = sizes.reduce((a, b) => a + b, 0)
        const gap = 3
        const usable = width - gap * (segs.length - 1)
        let x = 0
        for (let i = 0; i < segs.length; i++) {
            const s = segs[i]
            const w = Math.max(3, usable * sizes[i] / sum)
            box(ctx, x, w, trackColor, 1, 3)
            const frac = status === "complete" ? 1 : Math.min(1, s.received / Math.max(len(s), 1))
            if (frac > 0) {
                const done = s.state === "done" || frac >= 1
                box(ctx, x, w * frac, color, done ? 0.55 : 1, 3)
                if (showWriteHead && s.state === "receiving" && frac < 1) {
                    const head = Math.min(8, w * frac)
                    box(ctx, x + w * frac - head, head, "white", 0.35, 0)
                }
            }
            x += w + gap
        }
    }
}
