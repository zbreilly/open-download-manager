//! The app's one store, on the GUI thread.
use odm_ui::store::Store;
use std::cell::RefCell;

thread_local! {
    static STORE: RefCell<Store> = RefCell::new(Store::default());
}

pub fn with_store<R>(f: impl FnOnce(&Store) -> R) -> R {
    STORE.with(|s| f(&s.borrow()))
}

pub fn with_store_mut<R>(f: impl FnOnce(&mut Store) -> R) -> R {
    STORE.with(|s| f(&mut s.borrow_mut()))
}
