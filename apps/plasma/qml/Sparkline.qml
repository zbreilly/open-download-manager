import QtQuick
import QtQuick.Controls as QQC2
import QtQuick.Layouts
import org.kde.kirigami as Kirigami

// 60-second speed history with its peak.
ColumnLayout {
    id: spark

    // bytes per second, oldest first
    property var samples: []
    // "peak 4.1 MB/s", formatted by the view-model
    property string peak: ""
    property int graphHeight: 56

    spacing: Kirigami.Units.smallSpacing
    onSamplesChanged: canvas.requestPaint()

    RowLayout {
        Layout.fillWidth: true
        QQC2.Label {
            Layout.fillWidth: true
            text: "Speed, last 60 s"
            font: Kirigami.Theme.smallFont
            color: Kirigami.Theme.disabledTextColor
        }
        QQC2.Label {
            text: "peak " + spark.peak
            font: Kirigami.Theme.smallFont
            color: Kirigami.Theme.disabledTextColor
        }
    }

    Rectangle {
        Layout.fillWidth: true
        implicitHeight: spark.graphHeight
        radius: Kirigami.Units.cornerRadius
        color: Qt.alpha(Kirigami.Theme.textColor, 0.06)

        Canvas {
            id: canvas
            anchors.fill: parent
            onWidthChanged: requestPaint()
            onPaint: {
                const ctx = getContext("2d")
                ctx.reset()
                const s = spark.samples.slice(-60)
                if (s.length < 2)
                    return
                const peak = Math.max(1, ...s)
                const off = 60 - s.length
                const px = i => width * (off + i) / 59
                const py = i => height - 3 - s[i] / peak * (height - 8)
                ctx.beginPath()
                ctx.moveTo(px(0), py(0))
                for (let i = 1; i < s.length; i++)
                    ctx.lineTo(px(i), py(i))
                ctx.strokeStyle = Kirigami.Theme.highlightColor
                ctx.lineWidth = 1.6
                ctx.stroke()
                ctx.lineTo(px(s.length - 1), height)
                ctx.lineTo(px(0), height)
                ctx.closePath()
                ctx.globalAlpha = 0.18
                ctx.fillStyle = Kirigami.Theme.highlightColor
                ctx.fill()
                ctx.globalAlpha = 1
            }
        }
    }
}
