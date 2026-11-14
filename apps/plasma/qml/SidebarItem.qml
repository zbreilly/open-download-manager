import QtQuick
import QtQuick.Controls as QQC2
import QtQuick.Layouts
import org.kde.kirigami as Kirigami

// One line of the main window's sidebar: icon, name and a count.
QQC2.ItemDelegate {
    id: item

    property string key: ""
    property string iconName: ""
    property int count: 0

    Layout.fillWidth: true
    highlighted: checked
    checkable: true
    autoExclusive: true

    contentItem: RowLayout {
        spacing: Kirigami.Units.smallSpacing * 2
        Kirigami.Icon {
            source: item.iconName
            implicitWidth: Kirigami.Units.iconSizes.smallMedium
            implicitHeight: Kirigami.Units.iconSizes.smallMedium
        }
        QQC2.Label {
            textFormat: Text.PlainText
            Layout.fillWidth: true
            text: item.text
            elide: Text.ElideRight
            color: item.highlighted ? Kirigami.Theme.highlightedTextColor : Kirigami.Theme.textColor
        }
        QQC2.Label {
            textFormat: Text.PlainText
            visible: item.count > 0
            text: item.count
            font: Kirigami.Theme.smallFont
            color: item.highlighted ? Kirigami.Theme.highlightedTextColor : Kirigami.Theme.disabledTextColor
        }
    }
}
