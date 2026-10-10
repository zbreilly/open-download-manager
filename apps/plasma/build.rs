use cxx_qt_build::{CxxQtBuilder, QmlModule};

fn main() {
    let module = QmlModule::new("org.opendm.app")
        .qml_files([
            "qml/Main.qml",
            "qml/AddWindow.qml",
            "qml/ProgressWindow.qml",
            "qml/SchedulerWindow.qml",
            "qml/GrabberWindow.qml",
            "qml/SettingsWindow.qml",
            "qml/SegmentMap.qml",
            "qml/Sparkline.qml",
            "qml/SidebarItem.qml",
            "qml/StateField.qml",
            "qml/StateChoice.qml",
        ])
        ;
    CxxQtBuilder::new_qml_module(module)
        .files(["src/backend.rs", "src/model.rs", "src/vm.rs"])
        // QApplication (the Plasma style needs it), QQuickStyle
        .qt_module("Widgets")
        .qt_module("QuickControls2")
        .build();
    println!("cargo::rerun-if-changed=cpp/helpers.h");
}
