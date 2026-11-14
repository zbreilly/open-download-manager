import QtQuick
import QtQuick.Controls as QQC2

// A drop-down over [{ id, name }] whose choice belongs to the view-model.
QQC2.ComboBox {
    id: box

    // [{ id, name }]
    property var choices: []
    // the chosen id, from the view-model
    property string value: ""
    signal chosen(string id)

    function sync() {
        for (let i = 0; i < choices.length; i++) {
            if (String(choices[i].id) === value) {
                if (currentIndex !== i)
                    currentIndex = i
                return
            }
        }
    }

    model: choices
    textRole: "name"
    onChoicesChanged: sync()
    onValueChanged: sync()
    Component.onCompleted: sync()
    onActivated: index => box.chosen(String(choices[index].id))
}
