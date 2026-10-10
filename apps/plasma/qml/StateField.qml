import QtQuick
import QtQuick.Controls as QQC2

// A text field whose text belongs to the view-model: it shows `value` and
// reports edits, instead of keeping its own copy.
QQC2.TextField {
    id: field

    // what the view-model says the text is
    property string value: ""
    signal edited(string text)

    onValueChanged: if (text !== value) text = value
    Component.onCompleted: text = value
    onTextEdited: field.edited(text)
}
