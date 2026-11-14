//! The download list as Qt sees it: the rows of the current sidebar entry
//! that match the search, sorted. Filtering, sorting and formatting are
//! `odm-ui`'s; this exposes the result as a `QAbstractListModel`.
use crate::state::with_store;
use core::pin::Pin;
use cxx_qt::CxxQtType;
use cxx_qt_lib::{QByteArray, QHash, QHashPair_i32_QByteArray, QModelIndex, QString, QVariant};
use odm_ui::store::{Row, SortKey};

#[cxx_qt::bridge]
pub mod ffi {
    unsafe extern "C++" {
        include!(<QtCore/QAbstractListModel>);
        type QAbstractListModel;

        include!("cxx-qt-lib/qhash.h");
        type QHash_i32_QByteArray = cxx_qt_lib::QHash<cxx_qt_lib::QHashPair_i32_QByteArray>;
        include!("cxx-qt-lib/qvariant.h");
        type QVariant = cxx_qt_lib::QVariant;
        include!("cxx-qt-lib/qmodelindex.h");
        type QModelIndex = cxx_qt_lib::QModelIndex;
        include!("cxx-qt-lib/qstring.h");
        type QString = cxx_qt_lib::QString;
    }

    extern "RustQt" {
        #[qobject]
        #[base = QAbstractListModel]
        #[qml_element]
        #[qproperty(i32, count)]
        type DownloadModel = super::DownloadModelRust;

        #[cxx_override]
        fn data(self: &DownloadModel, index: &QModelIndex, role: i32) -> QVariant;

        #[cxx_override]
        #[cxx_name = "roleNames"]
        fn role_names(self: &DownloadModel) -> QHash_i32_QByteArray;

        #[cxx_override]
        #[cxx_name = "rowCount"]
        fn row_count(self: &DownloadModel, parent: &QModelIndex) -> i32;

        /// Show the rows of sidebar entry `filter` ("library:all",
        /// "category:video", "queue:main") matching `search`, sorted by
        /// `sort` ("name", "size", "progress", "speed", "eta", "status",
        /// "queue", "added").
        #[qinvokable]
        fn show(self: Pin<&mut DownloadModel>, filter: &QString, search: &QString, sort: &QString, descending: bool);

        /// The store changed: bring the rows up to date.
        #[qinvokable]
        fn refresh(self: Pin<&mut DownloadModel>);

        /// The id of the row at `row` ("" when out of range).
        #[qinvokable]
        #[cxx_name = "idAt"]
        fn id_at(self: &DownloadModel, row: i32) -> QString;

        /// The row of download `id` (-1 when it isn't shown).
        #[qinvokable]
        #[cxx_name = "rowOf"]
        fn row_of(self: &DownloadModel, id: &QString) -> i32;
    }

    unsafe extern "RustQt" {
        #[inherit]
        #[cxx_name = "beginResetModel"]
        unsafe fn begin_reset_model(self: Pin<&mut DownloadModel>);

        #[inherit]
        #[cxx_name = "endResetModel"]
        unsafe fn end_reset_model(self: Pin<&mut DownloadModel>);
    }
}

pub struct DownloadModelRust {
    count: i32,
    rows: Vec<Row>,
    filter: String,
    search: String,
    sort: SortKey,
    descending: bool,
}

impl Default for DownloadModelRust {
    fn default() -> Self {
        DownloadModelRust { count: 0, rows: vec![], filter: "library:all".into(), search: String::new(), sort: SortKey::Added, descending: true }
    }
}

const ROLES: [&str; 11] = ["id", "name", "size", "percent", "speed", "eta", "statusLabel", "status", "queue", "added", "category"];
/// `Qt::UserRole`
const FIRST_ROLE: i32 = 0x0100;

fn sort_key(name: &str) -> SortKey {
    match name {
        "name" => SortKey::Name,
        "size" => SortKey::Size,
        "progress" => SortKey::Progress,
        "speed" => SortKey::Speed,
        "eta" => SortKey::Eta,
        "status" => SortKey::Status,
        "queue" => SortKey::Queue,
        _ => SortKey::Added,
    }
}

impl ffi::DownloadModel {
    fn data(&self, index: &QModelIndex, role: i32) -> QVariant {
        let Some(row) = self.rust().rows.get(index.row() as usize) else { return QVariant::default() };
        let text = |s: &str| QVariant::from(&QString::from(s));
        match role - FIRST_ROLE {
            0 => text(&row.id),
            1 => text(&row.name),
            2 => text(&row.size),
            3 => QVariant::from(&(row.percent as i32)),
            4 => text(&row.speed),
            5 => text(&row.eta),
            6 => text(&row.status_label),
            7 => text(&row.status),
            8 => text(&row.queue),
            9 => text(&row.added),
            10 => text(&row.category),
            _ => QVariant::default(),
        }
    }

    fn role_names(&self) -> QHash<QHashPair_i32_QByteArray> {
        let mut roles = QHash::<QHashPair_i32_QByteArray>::default();
        for (i, name) in ROLES.iter().enumerate() {
            roles.insert(FIRST_ROLE + i as i32, QByteArray::from(*name));
        }
        roles
    }

    fn row_count(&self, _parent: &QModelIndex) -> i32 {
        self.rust().rows.len() as i32
    }

    fn show(mut self: Pin<&mut Self>, filter: &QString, search: &QString, sort: &QString, descending: bool) {
        {
            let mut r = self.as_mut().rust_mut();
            r.filter = filter.to_string();
            r.search = search.to_string();
            r.sort = sort_key(&sort.to_string());
            r.descending = descending;
        }
        self.refresh();
    }

    fn refresh(mut self: Pin<&mut Self>) {
        let rows: Vec<Row> = {
            let r = self.rust();
            with_store(|st| st.visible(&r.filter, &r.search, r.sort, r.descending).iter().map(|d| st.row(d)).collect())
        };
        if rows == self.rust().rows {
            return;
        }
        // rows are few enough on screen that a reset is cheap; the view
        // keeps its selection by id (see Main.qml)
        unsafe { self.as_mut().begin_reset_model() };
        let count = rows.len() as i32;
        self.as_mut().rust_mut().rows = rows;
        unsafe { self.as_mut().end_reset_model() };
        self.set_count(count);
    }

    fn id_at(&self, row: i32) -> QString {
        QString::from(self.rust().rows.get(row as usize).map(|r| r.id.as_str()).unwrap_or(""))
    }

    fn row_of(&self, id: &QString) -> i32 {
        let id = id.to_string();
        self.rust().rows.iter().position(|r| r.id == id).map(|i| i as i32).unwrap_or(-1)
    }
}
