//! Open Download Manager for KDE Plasma: a Kirigami (Qt Quick) interface on
//! the shared Rust engine (`odm_ffi::api`), with the desktop services from
//! `odm-linux`. The interface is QML (`qml/`); `backend.rs` is its bridge
//! to the engine.
mod backend;
mod model;
mod state;
mod vm;
mod vms;

use cxx_qt_lib::{QGuiApplication, QQmlApplicationEngine, QString, QUrl};
use cxx_qt_lib_extras::QApplication;
use odm_linux::shell::Shell;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    // before any Qt object exists: a second launch only hands its arguments
    // to the running instance
    match Shell::start(args, backend::shell_event) {
        Ok(Some(shell)) => backend::set_shell(shell),
        Ok(None) => return,
        Err(e) => {
            eprintln!("open-download-manager: {e}");
            std::process::exit(1);
        }
    }

    backend::ffi::use_desktop_style();
    let mut app = QApplication::new();
    if let Some(mut app) = app.as_mut() {
        app.as_mut().set_application_name(&QString::from("open-download-manager"));
        app.as_mut().set_application_display_name(&QString::from(odm_linux::APP_NAME));
        app.as_mut().set_application_version(&QString::from(env!("CARGO_PKG_VERSION")));
        app.as_mut().set_organization_domain(&QString::from("opendm.app"));
    }
    // ties the windows to the installed desktop file (icon, taskbar grouping)
    QGuiApplication::set_desktop_file_name(&QString::from(odm_linux::APP_ID));
    backend::ffi::set_window_icon(&QString::from(odm_linux::APP_ID));
    backend::ffi::keep_running_without_windows();

    let mut engine = QQmlApplicationEngine::new();
    if let Some(engine) = engine.as_mut() {
        engine.load(&QUrl::from("qrc:/qt/qml/org/opendm/app/qml/Main.qml"));
    }
    if let Some(app) = app.as_mut() {
        app.exec();
    }
    // also reached when the session ends: stop downloads and save state
    odm_ffi::api::shutdown();
}
