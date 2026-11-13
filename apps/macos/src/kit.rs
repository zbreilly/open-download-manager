//! A thin layer over AppKit: Rust closures as targets and delegates, table
//! sources, a drawing view, and the handful of layout helpers the windows
//! are built from. Everything here runs on the main thread.
use block2::RcBlock;
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, ProtocolObject};
use objc2::{define_class, msg_send, sel, DefinedClass, MainThreadMarker, MainThreadOnly, Message};
use objc2_app_kit::*;
use objc2_foundation::*;
use std::cell::{Cell, RefCell};
use std::ffi::c_void;

pub fn mtm() -> MainThreadMarker {
    MainThreadMarker::new().expect("the interface runs on the main thread")
}

pub fn ns(s: &str) -> Retained<NSString> {
    NSString::from_str(s)
}

/// Upcast to a plain view.
pub fn view(v: &NSView) -> Retained<NSView> {
    v.retain()
}

/// Keep `object` alive for as long as `owner` lives (targets and delegates
/// are weak references in AppKit).
pub fn keep(owner: &AnyObject, object: &AnyObject) {
    let key = object as *const AnyObject as *const c_void;
    unsafe {
        objc2::ffi::objc_setAssociatedObject(
            owner as *const AnyObject as *mut AnyObject,
            key,
            object as *const AnyObject as *mut AnyObject,
            objc2::ffi::OBJC_ASSOCIATION_RETAIN_NONATOMIC,
        );
    }
}

// ---------------------------------------------------------------------------
// closures as targets and delegates

type Action = Box<dyn Fn(Option<&AnyObject>)>;

pub struct HandlerIvars {
    f: Action,
}

define_class!(
    // SAFETY: NSObject has no subclassing requirements; no Drop.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "ODMHandler"]
    #[ivars = HandlerIvars]
    pub struct Handler;

    impl Handler {
        /// target/action
        #[unsafe(method(fire:))]
        fn fire(&self, sender: Option<&AnyObject>) {
            (self.ivars().f)(sender)
        }
    }

    unsafe impl NSObjectProtocol for Handler {}

    unsafe impl NSControlTextEditingDelegate for Handler {
        #[unsafe(method(controlTextDidChange:))]
        fn control_text_did_change(&self, n: &NSNotification) {
            let object = unsafe { n.object() };
            (self.ivars().f)(object.as_deref())
        }
    }

    unsafe impl NSTextFieldDelegate for Handler {}

    unsafe impl NSTextDelegate for Handler {
        #[unsafe(method(textDidChange:))]
        fn text_did_change(&self, n: &NSNotification) {
            let object = unsafe { n.object() };
            (self.ivars().f)(object.as_deref())
        }
    }

    unsafe impl NSTextViewDelegate for Handler {}

    unsafe impl NSWindowDelegate for Handler {
        #[unsafe(method(windowWillClose:))]
        fn window_will_close(&self, n: &NSNotification) {
            let object = unsafe { n.object() };
            (self.ivars().f)(object.as_deref())
        }
    }

    unsafe impl NSMenuDelegate for Handler {
        #[unsafe(method(menuNeedsUpdate:))]
        fn menu_needs_update(&self, menu: &NSMenu) {
            (self.ivars().f)(Some(menu))
        }
    }
);

impl Handler {
    pub fn new(f: impl Fn(Option<&AnyObject>) + 'static) -> Retained<Self> {
        let this = Self::alloc(mtm()).set_ivars(HandlerIvars { f: Box::new(f) });
        unsafe { msg_send![super(this), init] }
    }

    /// A handler that ignores its sender.
    pub fn simple(f: impl Fn() + 'static) -> Retained<Self> {
        Self::new(move |_| f())
    }
}

/// Make `f` the action of a control.
pub fn on_action(control: &NSControl, f: impl Fn() + 'static) {
    let h = Handler::simple(f);
    unsafe {
        control.setTarget(Some(&h));
        control.setAction(Some(sel!(fire:)));
    }
    keep(control, &h);
}

/// Call `f` whenever the text of a field changes.
pub fn on_text(field: &NSTextField, f: impl Fn() + 'static) {
    let h = Handler::simple(f);
    unsafe { field.setDelegate(Some(ProtocolObject::from_ref(&*h))) };
    keep(field, &h);
}

/// Call `f` whenever the text of a text view changes.
pub fn on_text_view(text: &NSTextView, f: impl Fn() + 'static) {
    let h = Handler::simple(f);
    unsafe { text.setDelegate(Some(ProtocolObject::from_ref(&*h))) };
    keep(text, &h);
}

/// Call `f` when a window is about to close.
pub fn on_close(window: &NSWindow, f: impl Fn() + 'static) {
    let h = Handler::simple(f);
    unsafe { window.setDelegate(Some(ProtocolObject::from_ref(&*h))) };
    keep(window, &h);
}

/// Run `f` once, `seconds` from now.
pub fn after(seconds: f64, f: impl Fn() + 'static) {
    let block = RcBlock::new(move |_: std::ptr::NonNull<NSTimer>| f());
    unsafe { NSTimer::scheduledTimerWithTimeInterval_repeats_block(seconds, false, &block) };
}

/// Run `f` every `seconds`, for the life of the app.
pub fn every(seconds: f64, f: impl Fn() + 'static) {
    let block = RcBlock::new(move |_: std::ptr::NonNull<NSTimer>| f());
    let timer = unsafe { NSTimer::timerWithTimeInterval_repeats_block(seconds, true, &block) };
    // common modes: keeps ticking while a menu is open or a window is dragged
    unsafe { NSRunLoop::mainRunLoop().addTimer_forMode(&timer, NSRunLoopCommonModes) };
}

// ---------------------------------------------------------------------------
// menus

pub fn menu(title: &str) -> Retained<NSMenu> {
    let m = unsafe { NSMenu::initWithTitle(NSMenu::alloc(mtm()), &ns(title)) };
    unsafe { m.setAutoenablesItems(false) };
    m
}

/// An item that runs `f`.
pub fn item(title: &str, enabled: bool, f: impl Fn() + 'static) -> Retained<NSMenuItem> {
    let i = unsafe { NSMenuItem::initWithTitle_action_keyEquivalent(NSMenuItem::alloc(mtm()), &ns(title), Some(sel!(fire:)), &ns("")) };
    let h = Handler::simple(f);
    unsafe {
        i.setTarget(Some(&h));
        i.setEnabled(enabled);
    }
    keep(&i, &h);
    i
}

/// An item with a key equivalent; `modifiers` in addition to nothing (pass
/// Command explicitly).
pub fn key_item(title: &str, key: &str, modifiers: NSEventModifierFlags, f: impl Fn() + 'static) -> Retained<NSMenuItem> {
    let i = item(title, true, f);
    unsafe {
        i.setKeyEquivalent(&ns(key));
        i.setKeyEquivalentModifierMask(modifiers);
    }
    i
}

/// An item that sends a standard action down the responder chain.
pub fn responder_item(title: &str, action: objc2::runtime::Sel, key: &str) -> Retained<NSMenuItem> {
    unsafe { NSMenuItem::initWithTitle_action_keyEquivalent(NSMenuItem::alloc(mtm()), &ns(title), Some(action), &ns(key)) }
}

pub fn separator() -> Retained<NSMenuItem> {
    NSMenuItem::separatorItem(mtm())
}

pub fn disabled_item(title: &str) -> Retained<NSMenuItem> {
    item(title, false, || {})
}

pub fn submenu(title: &str, items: Vec<Retained<NSMenuItem>>) -> Retained<NSMenuItem> {
    let m = menu(title);
    for i in &items {
        m.addItem(i);
    }
    let holder = disabled_item(title);
    unsafe {
        holder.setEnabled(true);
        holder.setSubmenu(Some(&m));
    }
    holder
}

pub fn checked(i: Retained<NSMenuItem>, on: bool) -> Retained<NSMenuItem> {
    unsafe { i.setState(if on { NSControlStateValueOn } else { NSControlStateValueOff }) };
    i
}

/// A menu rebuilt by `fill` every time it is about to open.
pub fn dynamic_menu(title: &str, fill: impl Fn(&NSMenu) + 'static) -> Retained<NSMenu> {
    let m = menu(title);
    let h = Handler::new(move |sender| {
        if let Some(menu) = sender.and_then(|s| s.downcast_ref::<NSMenu>()) {
            menu.removeAllItems();
            fill(menu);
        }
    });
    unsafe { m.setDelegate(Some(ProtocolObject::from_ref(&*h))) };
    keep(&m, &h);
    m
}

// ---------------------------------------------------------------------------
// controls

pub fn symbol(name: &str, description: &str) -> Option<Retained<NSImage>> {
    unsafe { NSImage::imageWithSystemSymbolName_accessibilityDescription(&ns(name), Some(&ns(description))) }
}

pub fn label(text: &str) -> Retained<NSTextField> {
    let l = unsafe { NSTextField::labelWithString(&ns(text), mtm()) };
    unsafe { l.setLineBreakMode(NSLineBreakMode::ByTruncatingMiddle) };
    l.setContentCompressionResistancePriority_forOrientation(250.0, NSLayoutConstraintOrientation::Horizontal);
    l
}

/// Secondary-colored label.
pub fn dim(text: &str) -> Retained<NSTextField> {
    let l = label(text);
    unsafe { l.setTextColor(Some(&NSColor::secondaryLabelColor())) };
    l
}

/// Small secondary text that wraps.
pub fn note(text: &str) -> Retained<NSTextField> {
    let l = unsafe { NSTextField::wrappingLabelWithString(&ns(text), mtm()) };
    unsafe {
        l.setFont(Some(&NSFont::systemFontOfSize(NSFont::smallSystemFontSize())));
        l.setTextColor(Some(&NSColor::secondaryLabelColor()));
        l.setSelectable(false);
    }
    l.setContentCompressionResistancePriority_forOrientation(250.0, NSLayoutConstraintOrientation::Horizontal);
    // wrapping text takes the width it is given
    l.setContentHuggingPriority_forOrientation(1.0, NSLayoutConstraintOrientation::Horizontal);
    l
}

pub fn mono(l: &NSTextField) {
    let size = unsafe { l.font().map(|f| f.pointSize()).unwrap_or(13.0) };
    unsafe { l.setFont(Some(&NSFont::monospacedSystemFontOfSize_weight(size - 1.0, NSFontWeightRegular))) };
}

/// Digits of equal width, so numbers don't jitter as they change.
pub fn digits(l: &NSTextField) {
    let size = unsafe { l.font().map(|f| f.pointSize()).unwrap_or(13.0) };
    unsafe { l.setFont(Some(&NSFont::monospacedDigitSystemFontOfSize_weight(size, NSFontWeightRegular))) };
}

pub fn set_font(l: &NSTextField, size: f64, bold: bool) {
    let weight = if bold { unsafe { NSFontWeightSemibold } } else { unsafe { NSFontWeightRegular } };
    unsafe { l.setFont(Some(&NSFont::systemFontOfSize_weight(size, weight))) };
}

pub fn set_text(l: &NSTextField, text: &str) {
    // leave an unchanged field alone (keeps selections and avoids redraws)
    if unsafe { l.stringValue() }.to_string() != text {
        unsafe { l.setStringValue(&ns(text)) };
    }
}

pub fn text_of(l: &NSTextField) -> String {
    unsafe { l.stringValue() }.to_string()
}


pub fn button(title: &str, f: impl Fn() + 'static) -> Retained<NSButton> {
    let b = unsafe { NSButton::buttonWithTitle_target_action(&ns(title), None, None, mtm()) };
    on_action(&b, f);
    b
}

/// A borderless button showing an SF Symbol.
pub fn icon_button(symbol_name: &str, description: &str, f: impl Fn() + 'static) -> Retained<NSButton> {
    let b = button("", f);
    unsafe {
        b.setImage(symbol(symbol_name, description).as_deref());
        b.setBordered(false);
        b.setToolTip(Some(&ns(description)));
    }
    b
}

/// The button Return presses.
pub fn default_button(b: &NSButton) {
    unsafe { b.setKeyEquivalent(&ns("\r")) };
}

/// The button Escape presses.
pub fn cancel_button(b: &NSButton) {
    unsafe { b.setKeyEquivalent(&ns("\u{1b}")) };
}

pub fn checkbox(title: &str, f: impl Fn() + 'static) -> Retained<NSButton> {
    let b = unsafe { NSButton::checkboxWithTitle_target_action(&ns(title), None, None, mtm()) };
    on_action(&b, f);
    b
}

pub fn is_on(b: &NSButton) -> bool {
    unsafe { b.state() == NSControlStateValueOn }
}

pub fn set_on(b: &NSButton, on: bool) {
    unsafe { b.setState(if on { NSControlStateValueOn } else { NSControlStateValueOff }) };
}

pub fn text_field(placeholder: &str, f: impl Fn() + 'static) -> Retained<NSTextField> {
    let t = unsafe { NSTextField::textFieldWithString(&ns(""), mtm()) };
    unsafe { t.setPlaceholderString(Some(&ns(placeholder))) };
    unsafe { t.setLineBreakMode(NSLineBreakMode::ByTruncatingTail) };
    t.setContentHuggingPriority_forOrientation(100.0, NSLayoutConstraintOrientation::Horizontal);
    t.setContentCompressionResistancePriority_forOrientation(200.0, NSLayoutConstraintOrientation::Horizontal);
    on_text(&t, f);
    t
}

pub fn secure_field(placeholder: &str, f: impl Fn() + 'static) -> Retained<NSSecureTextField> {
    let t = NSSecureTextField::new(mtm());
    unsafe { t.setPlaceholderString(Some(&ns(placeholder))) };
    t.setContentHuggingPriority_forOrientation(100.0, NSLayoutConstraintOrientation::Horizontal);
    on_text(&t, f);
    t
}

/// A pop-up button over `titles`; `f` runs when the choice changes.
pub fn popup(titles: &[String], f: impl Fn() + 'static) -> Retained<NSPopUpButton> {
    let p = unsafe { NSPopUpButton::initWithFrame_pullsDown(NSPopUpButton::alloc(mtm()), NSRect::ZERO, false) };
    set_popup(&p, titles, 0);
    on_action(&p, f);
    p
}

pub fn set_popup(p: &NSPopUpButton, titles: &[String], selected: usize) {
    unsafe {
        p.removeAllItems();
        for t in titles {
            // through the menu: addItemWithTitle drops duplicate titles
            let i = NSMenuItem::initWithTitle_action_keyEquivalent(NSMenuItem::alloc(mtm()), &ns(t), None, &ns(""));
            if let Some(m) = p.menu() {
                m.addItem(&i);
            }
        }
        if selected < titles.len() {
            p.selectItemAtIndex(selected as isize);
        }
    }
}

pub fn popup_index(p: &NSPopUpButton) -> usize {
    unsafe { p.indexOfSelectedItem() }.max(0) as usize
}

pub fn segmented(labels: &[&str], f: impl Fn() + 'static) -> Retained<NSSegmentedControl> {
    let s = NSSegmentedControl::new(mtm());
    unsafe {
        s.setSegmentCount(labels.len() as isize);
        for (i, l) in labels.iter().enumerate() {
            s.setLabel_forSegment(&ns(l), i as isize);
        }
        s.setTrackingMode(NSSegmentSwitchTracking::SelectOne);
    }
    on_action(&s, f);
    s
}

/// A multi-line editor in a scroll view: `(scroll view, text view)`.
pub fn text_area(f: impl Fn() + 'static) -> (Retained<NSScrollView>, Retained<NSTextView>) {
    let scroll = unsafe { NSTextView::scrollableTextView(mtm()) };
    let text = unsafe { scroll.documentView() }.and_then(|d| d.downcast::<NSTextView>().ok()).expect("a text view");
    unsafe {
        text.setFont(Some(&NSFont::monospacedSystemFontOfSize_weight(12.0, NSFontWeightRegular)));
        text.setRichText(false);
        text.setAutomaticQuoteSubstitutionEnabled(false);
        text.setAutomaticDashSubstitutionEnabled(false);
        text.setAutomaticTextReplacementEnabled(false);
        text.setAutomaticSpellingCorrectionEnabled(false);
        scroll.setBorderType(NSBorderType::BezelBorder);
    }
    on_text_view(&text, f);
    (scroll, text)
}

pub fn text_view_string(t: &NSTextView) -> String {
    unsafe { t.string() }.to_string()
}

// ---------------------------------------------------------------------------
// layout

pub fn stack(vertical: bool, spacing: f64, views: &[&NSView]) -> Retained<NSStackView> {
    let s = NSStackView::new(mtm());
    unsafe {
        s.setOrientation(if vertical { NSUserInterfaceLayoutOrientation::Vertical } else { NSUserInterfaceLayoutOrientation::Horizontal });
        s.setSpacing(spacing);
        s.setAlignment(if vertical { NSLayoutAttribute::Leading } else { NSLayoutAttribute::CenterY });
        for v in views {
            s.addArrangedSubview(v);
        }
    }
    s
}

pub fn vstack(spacing: f64, views: &[&NSView]) -> Retained<NSStackView> {
    stack(true, spacing, views)
}

pub fn hstack(spacing: f64, views: &[&NSView]) -> Retained<NSStackView> {
    stack(false, spacing, views)
}

pub fn pad(s: &NSStackView, top: f64, left: f64, bottom: f64, right: f64) {
    unsafe { s.setEdgeInsets(NSEdgeInsets { top, left, bottom, right }) };
}

/// An invisible view that takes up the slack in a stack.
pub fn spacer() -> Retained<NSView> {
    let v = NSView::new(mtm());
    v.setContentHuggingPriority_forOrientation(1.0, NSLayoutConstraintOrientation::Horizontal);
    v.setContentHuggingPriority_forOrientation(1.0, NSLayoutConstraintOrientation::Vertical);
    v
}

pub fn hline() -> Retained<NSBox> {
    let b = NSBox::new(mtm());
    unsafe { b.setBoxType(NSBoxType::Separator) };
    b
}

pub fn width(v: &NSView, w: f64) {
    unsafe { v.widthAnchor().constraintEqualToConstant(w).setActive(true) };
}


pub fn height(v: &NSView, h: f64) {
    unsafe { v.heightAnchor().constraintEqualToConstant(h).setActive(true) };
}

pub fn min_height(v: &NSView, h: f64) {
    unsafe { v.heightAnchor().constraintGreaterThanOrEqualToConstant(h).setActive(true) };
}


/// Put `child` in `parent`, filling it.
pub fn fill(parent: &NSView, child: &NSView) {
    unsafe {
        child.setTranslatesAutoresizingMaskIntoConstraints(false);
        parent.addSubview(child);
        child.topAnchor().constraintEqualToAnchor(&parent.topAnchor()).setActive(true);
        child.bottomAnchor().constraintEqualToAnchor(&parent.bottomAnchor()).setActive(true);
        child.leadingAnchor().constraintEqualToAnchor(&parent.leadingAnchor()).setActive(true);
        child.trailingAnchor().constraintEqualToAnchor(&parent.trailingAnchor()).setActive(true);
    }
}

/// A two-column form: labels right-aligned on the left, controls on the
/// right. A row with an empty label holds its control alone.
pub fn form(rows: &[(&str, &NSView)]) -> Retained<NSGridView> {
    let g = unsafe { NSGridView::gridViewWithNumberOfColumns_rows(2, 0, mtm()) };
    unsafe {
        for (title, control) in rows {
            let l = label(title);
            let cells = NSArray::from_retained_slice(&[view(&l), view(control)]);
            g.addRowWithViews(&cells);
        }
        g.columnAtIndex(0).setXPlacement(NSGridCellPlacement::Trailing);
        g.setRowAlignment(NSGridRowAlignment::FirstBaseline);
        g.setRowSpacing(8.0);
        g.setColumnSpacing(10.0);
    }
    g.setContentHuggingPriority_forOrientation(600.0, NSLayoutConstraintOrientation::Vertical);
    g
}

pub fn show(v: &NSView, visible: bool) {
    if unsafe { v.isHidden() } == visible {
        unsafe { v.setHidden(!visible) };
    }
}

pub fn enable(c: &NSControl, on: bool) {
    if unsafe { c.isEnabled() } != on {
        unsafe { c.setEnabled(on) };
    }
}

/// A view scrolled vertically inside a scroll view (for long forms).
pub fn scrolling(content: &NSView) -> Retained<NSScrollView> {
    let scroll = NSScrollView::new(mtm());
    let clip = FlippedView::new();
    unsafe {
        scroll.setHasVerticalScroller(true);
        scroll.setAutohidesScrollers(true);
        scroll.setDrawsBackground(false);
        scroll.setBorderType(NSBorderType::NoBorder);
        content.setTranslatesAutoresizingMaskIntoConstraints(false);
        clip.setTranslatesAutoresizingMaskIntoConstraints(false);
        clip.addSubview(content);
        content.topAnchor().constraintEqualToAnchor(&clip.topAnchor()).setActive(true);
        content.leadingAnchor().constraintEqualToAnchor(&clip.leadingAnchor()).setActive(true);
        content.trailingAnchor().constraintEqualToAnchor(&clip.trailingAnchor()).setActive(true);
        content.bottomAnchor().constraintEqualToAnchor(&clip.bottomAnchor()).setActive(true);
        scroll.setDocumentView(Some(&clip));
        let cv = scroll.contentView();
        clip.topAnchor().constraintEqualToAnchor(&cv.topAnchor()).setActive(true);
        clip.leadingAnchor().constraintEqualToAnchor(&cv.leadingAnchor()).setActive(true);
        clip.trailingAnchor().constraintEqualToAnchor(&cv.trailingAnchor()).setActive(true);
    }
    scroll
}

// ---------------------------------------------------------------------------
// windows, alerts, panels

pub fn window(title: &str, w: f64, h: f64, min: (f64, f64), resizable: bool) -> Retained<NSWindow> {
    let mut style = NSWindowStyleMask::Titled | NSWindowStyleMask::Closable | NSWindowStyleMask::Miniaturizable;
    if resizable {
        style |= NSWindowStyleMask::Resizable;
    }
    let win = unsafe {
        NSWindow::initWithContentRect_styleMask_backing_defer(
            NSWindow::alloc(mtm()),
            NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(w, h)),
            style,
            NSBackingStoreType::Buffered,
            false,
        )
    };
    unsafe {
        // the Rust side owns the window
        win.setReleasedWhenClosed(false);
        win.setTitle(&ns(title));
        win.setContentMinSize(NSSize::new(min.0, min.1));
        win.center();
    }
    win
}

/// Debug builds with `ODM_SHOTS` set are being checked through window
/// snapshots: they stay behind everything and never take the keyboard.
pub fn quiet() -> bool {
    cfg!(debug_assertions) && std::env::var_os("ODM_SHOTS").is_some()
}

pub fn present(win: &NSWindow) {
    if quiet() {
        return unsafe { win.orderBack(None) };
    }
    let app = NSApplication::sharedApplication(mtm());
    #[allow(deprecated)]
    unsafe {
        app.activateIgnoringOtherApps(true)
    };
    win.makeKeyAndOrderFront(None);
}

/// A modal question. Returns the index of the button chosen.
pub fn ask(parent: Option<&NSWindow>, title: &str, text: &str, buttons: &[&str], destructive: Option<usize>) -> usize {
    let _ = parent;
    let a = NSAlert::new(mtm());
    unsafe {
        a.setMessageText(&ns(title));
        a.setInformativeText(&ns(text));
        for (i, b) in buttons.iter().enumerate() {
            let btn = a.addButtonWithTitle(&ns(b));
            if destructive == Some(i) {
                btn.setHasDestructiveAction(true);
            }
        }
        let r = a.runModal();
        (r - NSAlertFirstButtonReturn).max(0) as usize
    }
}

pub fn tell(title: &str, text: &str) {
    ask(None, title, text, &["OK"], None);
}

/// A modal question with one text field. None when cancelled.
pub fn prompt(title: &str, text: &str, value: &str, ok: &str) -> Option<String> {
    let a = NSAlert::new(mtm());
    let field = unsafe { NSTextField::textFieldWithString(&ns(value), mtm()) };
    unsafe {
        field.setFrame(NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(320.0, 24.0)));
        a.setMessageText(&ns(title));
        a.setInformativeText(&ns(text));
        a.addButtonWithTitle(&ns(ok));
        a.addButtonWithTitle(&ns("Cancel"));
        a.setAccessoryView(Some(&field));
        a.window().setInitialFirstResponder(Some(&field));
        (a.runModal() == NSAlertFirstButtonReturn).then(|| text_of(&field))
    }
}

pub fn choose_folder(start: &str) -> Option<String> {
    let p = unsafe { NSOpenPanel::openPanel(mtm()) };
    unsafe {
        p.setCanChooseDirectories(true);
        p.setCanChooseFiles(false);
        p.setCanCreateDirectories(true);
        p.setAllowsMultipleSelection(false);
        if !start.is_empty() {
            p.setDirectoryURL(Some(&NSURL::fileURLWithPath_isDirectory(&ns(start), true)));
        }
        if p.runModal() != NSModalResponseOK {
            return None;
        }
        p.URL().and_then(|u| u.path()).map(|s| s.to_string())
    }
}

// ---------------------------------------------------------------------------
// a view that draws with a closure

pub struct CanvasIvars {
    draw: Box<dyn Fn(NSRect)>,
}

define_class!(
    // SAFETY: NSView allows subclassing; no Drop.
    #[unsafe(super(NSView, NSResponder, NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "ODMCanvas"]
    #[ivars = CanvasIvars]
    pub struct Canvas;

    impl Canvas {
        #[unsafe(method(drawRect:))]
        fn draw_rect(&self, _dirty: NSRect) {
            (self.ivars().draw)(self.bounds())
        }

        #[unsafe(method(isFlipped))]
        fn is_flipped(&self) -> bool {
            true
        }
    }
);

impl Canvas {
    /// `draw(bounds)` paints the view; the origin is the top-left corner.
    pub fn new(draw: impl Fn(NSRect) + 'static) -> Retained<Self> {
        let this = Self::alloc(mtm()).set_ivars(CanvasIvars { draw: Box::new(draw) });
        unsafe { msg_send![super(this), initWithFrame: NSRect::ZERO] }
    }

    pub fn redraw(&self) {
        unsafe { self.setNeedsDisplay(true) };
    }
}

define_class!(
    /// A plain view whose origin is the top-left corner (document view of
    /// scrolling forms, so short content sits at the top).
    #[unsafe(super(NSView, NSResponder, NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "ODMFlippedView"]
    pub struct FlippedView;

    impl FlippedView {
        #[unsafe(method(isFlipped))]
        fn is_flipped(&self) -> bool {
            true
        }
    }
);

impl FlippedView {
    pub fn new() -> Retained<Self> {
        unsafe { msg_send![Self::alloc(mtm()), initWithFrame: NSRect::ZERO] }
    }
}

pub fn fill_round(rect: NSRect, radius: f64, color: &NSColor) {
    unsafe {
        color.setFill();
        NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(rect, radius, radius).fill();
    }
}

// ---------------------------------------------------------------------------
// tables

type SortFn = Box<dyn Fn(&str, bool)>;
type KeyFn = Box<dyn Fn(u16) -> bool>;
type DropFn = Box<dyn Fn(Vec<String>)>;
type CellFn = Box<dyn Fn(&NSTableView, &str, usize) -> Option<Retained<NSView>>>;

/// What a table asks of its owner. Rows are numbered from 0.
pub struct TableCallbacks {
    pub rows: Box<dyn Fn() -> usize>,
    /// the view for a cell: `(table, column identifier, row)`
    pub cell: CellFn,
    pub is_group: Box<dyn Fn(usize) -> bool>,
    pub selection_changed: Box<dyn Fn()>,
    /// `(column identifier, ascending)`
    pub sort_changed: SortFn,
    /// the user dragged a row: `(from, to)`, `to` counted before the move
    pub move_row: Option<Box<dyn Fn(usize, usize)>>,
}

impl Default for TableCallbacks {
    fn default() -> Self {
        TableCallbacks {
            rows: Box::new(|| 0),
            cell: Box::new(|_, _, _| None),
            is_group: Box::new(|_| false),
            selection_changed: Box::new(|| {}),
            sort_changed: Box::new(|_, _| {}),
            move_row: None,
        }
    }
}

define_class!(
    // SAFETY: NSObject has no subclassing requirements; no Drop.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "ODMTableSource"]
    #[ivars = TableCallbacks]
    pub struct TableSource;

    unsafe impl NSObjectProtocol for TableSource {}

    unsafe impl NSTableViewDataSource for TableSource {
        #[unsafe(method(numberOfRowsInTableView:))]
        fn rows(&self, _table: &NSTableView) -> isize {
            (self.ivars().rows)() as isize
        }

        #[unsafe(method(tableView:sortDescriptorsDidChange:))]
        fn sort_changed(&self, table: &NSTableView, _old: &NSArray<NSSortDescriptor>) {
            let first = unsafe { table.sortDescriptors() }.firstObject();
            if let Some(d) = first {
                let key = unsafe { d.key() }.map(|k| k.to_string()).unwrap_or_default();
                (self.ivars().sort_changed)(&key, unsafe { d.ascending() });
            }
        }

        #[unsafe(method_id(tableView:pasteboardWriterForRow:))]
        fn writer(&self, _table: &NSTableView, row: isize) -> Option<Retained<NSString>> {
            self.ivars().move_row.as_ref().map(|_| ns(&row.to_string()))
        }

        #[unsafe(method(tableView:validateDrop:proposedRow:proposedDropOperation:))]
        fn validate_drop(&self, table: &NSTableView, _info: &AnyObject, row: isize, _op: NSTableViewDropOperation) -> NSDragOperation {
            if self.ivars().move_row.is_some() {
                // between rows, never onto one
                unsafe { table.setDropRow_dropOperation(row, NSTableViewDropOperation::Above) };
                NSDragOperation::Move
            } else {
                NSDragOperation::None
            }
        }

        #[unsafe(method(tableView:acceptDrop:row:dropOperation:))]
        fn accept_drop(&self, _table: &NSTableView, info: &AnyObject, row: isize, _op: NSTableViewDropOperation) -> bool {
            let pasteboard: Retained<NSPasteboard> = unsafe { msg_send![info, draggingPasteboard] };
            let from = unsafe { pasteboard.stringForType(NSPasteboardTypeString) }.and_then(|s| s.to_string().parse::<usize>().ok());
            match (&self.ivars().move_row, from) {
                (Some(mv), Some(from)) if row >= 0 => {
                    mv(from, row as usize);
                    true
                }
                _ => false,
            }
        }
    }

    unsafe impl NSControlTextEditingDelegate for TableSource {}

    unsafe impl NSTableViewDelegate for TableSource {
        #[unsafe(method_id(tableView:viewForTableColumn:row:))]
        fn cell(&self, table: &NSTableView, column: Option<&NSTableColumn>, row: isize) -> Option<Retained<NSView>> {
            let id = column.map(|c| unsafe { c.identifier() }.to_string()).unwrap_or_default();
            (self.ivars().cell)(table, &id, row.max(0) as usize)
        }

        #[unsafe(method(tableView:isGroupRow:))]
        fn is_group(&self, _table: &NSTableView, row: isize) -> bool {
            (self.ivars().is_group)(row.max(0) as usize)
        }

        #[unsafe(method(tableView:shouldSelectRow:))]
        fn should_select(&self, _table: &NSTableView, row: isize) -> bool {
            !(self.ivars().is_group)(row.max(0) as usize)
        }

        #[unsafe(method(tableViewSelectionDidChange:))]
        fn selection_changed(&self, _n: &NSNotification) {
            (self.ivars().selection_changed)()
        }
    }
);

impl TableSource {
    pub fn new(callbacks: TableCallbacks) -> Retained<Self> {
        let this = Self::alloc(mtm()).set_ivars(callbacks);
        unsafe { msg_send![super(this), init] }
    }
}

pub struct KeyTableIvars {
    /// a key was pressed (its key code); true when handled
    key: RefCell<Option<KeyFn>>,
    /// rebuilding its rows: selection changes are not the user's
    pub loading: Cell<bool>,
}

define_class!(
    /// A table view that offers key presses to its owner (Delete, Return).
    #[unsafe(super(NSTableView, NSControl, NSView, NSResponder, NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "ODMTableView"]
    #[ivars = KeyTableIvars]
    pub struct KeyTable;

    impl KeyTable {
        #[unsafe(method(keyDown:))]
        fn key_down(&self, event: &NSEvent) {
            let code = unsafe { event.keyCode() };
            let handled = self.ivars().key.borrow().as_ref().map(|f| f(code)).unwrap_or(false);
            if !handled {
                unsafe { msg_send![super(self), keyDown: event] }
            }
        }
    }
);

pub const KEY_RETURN: u16 = 36;
pub const KEY_DELETE: u16 = 51;
pub const KEY_FORWARD_DELETE: u16 = 117;

impl KeyTable {
    pub fn new() -> Retained<Self> {
        let this = Self::alloc(mtm()).set_ivars(KeyTableIvars { key: RefCell::new(None), loading: Cell::new(false) });
        unsafe { msg_send![super(this), initWithFrame: NSRect::ZERO] }
    }

    pub fn on_key(&self, f: impl Fn(u16) -> bool + 'static) {
        *self.ivars().key.borrow_mut() = Some(Box::new(f));
    }

    pub fn loading(&self) -> bool {
        self.ivars().loading.get()
    }

    /// Reload every row, keeping the selection out of the owner's sight.
    pub fn reload(&self) {
        self.ivars().loading.set(true);
        unsafe { self.reloadData() };
        self.ivars().loading.set(false);
    }

    /// Select exactly these rows.
    pub fn select(&self, rows: &[usize]) {
        self.ivars().loading.set(true);
        let set = NSMutableIndexSet::new();
        for r in rows {
            set.addIndex(*r);
        }
        unsafe { self.selectRowIndexes_byExtendingSelection(&set, false) };
        self.ivars().loading.set(false);
    }

    pub fn selected(&self) -> Vec<usize> {
        let set = unsafe { self.selectedRowIndexes() };
        let mut out = vec![];
        let mut i = set.firstIndex();
        while i != NSNotFound as usize {
            out.push(i);
            i = set.indexGreaterThanIndex(i);
        }
        out
    }
}

/// A table in a scroll view, fed by `callbacks`.
/// `columns`: `(identifier, title, width, min width, sortable)`; a table
/// without titles has no header.
pub fn table(columns: &[(&str, &str, f64, f64, bool)], row_height: f64, callbacks: TableCallbacks) -> (Retained<NSScrollView>, Retained<KeyTable>) {
    let t = KeyTable::new();
    let source = TableSource::new(callbacks);
    unsafe {
        for (id, title, w, min, sortable) in columns {
            let c = NSTableColumn::initWithIdentifier(NSTableColumn::alloc(mtm()), &ns(id));
            c.setTitle(&ns(title));
            c.setWidth(*w);
            c.setMinWidth(*min);
            if *sortable {
                let d = NSSortDescriptor::sortDescriptorWithKey_ascending(Some(&ns(id)), true);
                c.setSortDescriptorPrototype(Some(&d));
            }
            t.addTableColumn(&c);
        }
        if columns.iter().all(|c| c.1.is_empty()) {
            t.setHeaderView(None);
        }
        t.setRowHeight(row_height);
        t.setUsesAlternatingRowBackgroundColors(false);
        t.setColumnAutoresizingStyle(NSTableViewColumnAutoresizingStyle::FirstColumnOnlyAutoresizingStyle);
        t.setDataSource(Some(ProtocolObject::from_ref(&*source)));
        t.setDelegate(Some(ProtocolObject::from_ref(&*source)));
    }
    keep(&t, &source);
    let scroll = NSScrollView::new(mtm());
    unsafe {
        scroll.setDocumentView(Some(&t));
        scroll.setHasVerticalScroller(true);
        scroll.setHasHorizontalScroller(true);
        scroll.setAutohidesScrollers(true);
        scroll.setBorderType(NSBorderType::NoBorder);
    }
    (scroll, t)
}

/// Allow rows to be dragged to a new position (see
/// [`TableCallbacks::move_row`]).
pub fn allow_reorder(t: &NSTableView) {
    unsafe {
        t.registerForDraggedTypes(&NSArray::from_slice(&[NSPasteboardTypeString]));
        t.setDraggingSourceOperationMask_forLocal(NSDragOperation::Move, true);
    }
}

/// A recycled cell holding one line of text (and optionally an icon):
/// `(cell, its text field, its image view)`.
pub fn text_cell(table: &NSTableView, kind: &str, with_image: bool) -> (Retained<NSTableCellView>, Retained<NSTextField>, Option<Retained<NSImageView>>) {
    let ident = ns(kind);
    let reused = unsafe { table.makeViewWithIdentifier_owner(&ident, None) }.and_then(|v| v.downcast::<NSTableCellView>().ok());
    if let Some(cell) = reused {
        if let Some(text) = unsafe { cell.textField() } {
            let image = unsafe { cell.imageView() };
            return (cell, text, image);
        }
    }
    let cell = NSTableCellView::new(mtm());
    let text = label("");
    unsafe {
        cell.setIdentifier(Some(&ident));
        text.setTranslatesAutoresizingMaskIntoConstraints(false);
        cell.addSubview(&text);
        cell.setTextField(Some(&text));
        text.centerYAnchor().constraintEqualToAnchor(&cell.centerYAnchor()).setActive(true);
        text.trailingAnchor().constraintEqualToAnchor_constant(&cell.trailingAnchor(), -2.0).setActive(true);
    }
    let image = with_image.then(|| {
        let iv = NSImageView::new(mtm());
        unsafe {
            iv.setTranslatesAutoresizingMaskIntoConstraints(false);
            cell.addSubview(&iv);
            cell.setImageView(Some(&iv));
            iv.leadingAnchor().constraintEqualToAnchor_constant(&cell.leadingAnchor(), 2.0).setActive(true);
            iv.centerYAnchor().constraintEqualToAnchor(&cell.centerYAnchor()).setActive(true);
            iv.widthAnchor().constraintEqualToConstant(18.0).setActive(true);
            iv.heightAnchor().constraintEqualToConstant(18.0).setActive(true);
            text.leadingAnchor().constraintEqualToAnchor_constant(&iv.trailingAnchor(), 6.0).setActive(true);
        }
        iv
    });
    if image.is_none() {
        unsafe { text.leadingAnchor().constraintEqualToAnchor_constant(&cell.leadingAnchor(), 2.0).setActive(true) };
    }
    (cell, text, image)
}

/// "● Label" with the dot in `color`.
pub fn dotted(text: &str, color: &NSColor) -> Retained<NSAttributedString> {
    let s = NSMutableAttributedString::from_nsstring(&ns(&format!("●  {text}")));
    unsafe {
        s.addAttribute_value_range(NSForegroundColorAttributeName, color, NSRange::new(0, 1));
        s.addAttribute_value_range(NSFontAttributeName, &NSFont::systemFontOfSize(8.0), NSRange::new(0, 1));
        s.addAttribute_value_range(NSBaselineOffsetAttributeName, &NSNumber::new_f64(1.5), NSRange::new(0, 1));
    }
    Retained::into_super(s)
}

/// The color that stands for a download status.
pub fn status_color(status: &str) -> Retained<NSColor> {
    unsafe {
        match status {
            "downloading" | "connecting" | "assembling" => NSColor::controlAccentColor(),
            "paused" => NSColor::systemOrangeColor(),
            "error" => NSColor::systemRedColor(),
            "complete" => NSColor::systemGreenColor(),
            _ => NSColor::secondaryLabelColor(),
        }
    }
}

/// A vertical stack whose rows are as wide as the stack.
pub fn vfill(spacing: f64, views: &[&NSView]) -> Retained<NSStackView> {
    vfill_in(spacing, (0.0, 0.0, 0.0, 0.0), views)
}

/// A vertical stack whose rows fill its width, inside margins
/// `(top, left, bottom, right)`.
pub fn vfill_in(spacing: f64, margins: (f64, f64, f64, f64), views: &[&NSView]) -> Retained<NSStackView> {
    let s = stack(true, spacing, &[]);
    pad(&s, margins.0, margins.1, margins.2, margins.3);
    for v in views {
        add_row_in(&s, v, margins.1, margins.3);
    }
    s
}

/// Add a full-width row to a [`vfill`] stack.
pub fn add_row(s: &NSStackView, v: &NSView) {
    add_row_in(s, v, 0.0, 0.0);
}

fn add_row_in(s: &NSStackView, v: &NSView, left: f64, right: f64) {
    unsafe {
        s.addArrangedSubview(v);
        v.leadingAnchor().constraintEqualToAnchor_constant(&s.leadingAnchor(), left).setActive(true);
        v.trailingAnchor().constraintEqualToAnchor_constant(&s.trailingAnchor(), -right).setActive(true);
    }
}

// ---------------------------------------------------------------------------
// toolbar

pub struct ToolbarIvars {
    order: Vec<String>,
    items: Vec<(String, Retained<NSToolbarItem>)>,
}

define_class!(
    // SAFETY: NSObject has no subclassing requirements; no Drop.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "ODMToolbarSource"]
    #[ivars = ToolbarIvars]
    pub struct ToolbarSource;

    unsafe impl NSObjectProtocol for ToolbarSource {}

    unsafe impl NSToolbarDelegate for ToolbarSource {
        #[unsafe(method_id(toolbar:itemForItemIdentifier:willBeInsertedIntoToolbar:))]
        fn item(&self, _toolbar: &NSToolbar, id: &NSString, _inserting: bool) -> Option<Retained<NSToolbarItem>> {
            let id = id.to_string();
            self.ivars().items.iter().find(|(k, _)| *k == id).map(|(_, i)| i.clone())
        }

        #[unsafe(method_id(toolbarDefaultItemIdentifiers:))]
        fn default_ids(&self, _toolbar: &NSToolbar) -> Retained<NSArray<NSString>> {
            self.ids()
        }

        #[unsafe(method_id(toolbarAllowedItemIdentifiers:))]
        fn allowed_ids(&self, _toolbar: &NSToolbar) -> Retained<NSArray<NSString>> {
            self.ids()
        }
    }
);

impl ToolbarSource {
    fn ids(&self) -> Retained<NSArray<NSString>> {
        let ids: Vec<Retained<NSString>> = self.ivars().order.iter().map(|s| ns(s)).collect();
        NSArray::from_retained_slice(&ids)
    }
}

/// A toolbar showing `order` (identifiers; system ones such as the flexible
/// space included), with `items` supplying the app's own.
pub fn toolbar(name: &str, order: &[&str], items: Vec<(String, Retained<NSToolbarItem>)>) -> Retained<NSToolbar> {
    let source = ToolbarSource::alloc(mtm()).set_ivars(ToolbarIvars { order: order.iter().map(|s| s.to_string()).collect(), items });
    let source: Retained<ToolbarSource> = unsafe { msg_send![super(source), init] };
    let t = unsafe { NSToolbar::initWithIdentifier(NSToolbar::alloc(mtm()), &ns(name)) };
    unsafe {
        t.setDelegate(Some(ProtocolObject::from_ref(&*source)));
        t.setDisplayMode(NSToolbarDisplayMode::IconOnly);
        t.setAllowsUserCustomization(false);
    }
    keep(&t, &source);
    t
}

/// A toolbar button showing an SF Symbol.
pub fn tool(id: &str, label: &str, symbol_name: &str, tip: &str, f: impl Fn() + 'static) -> Retained<NSToolbarItem> {
    let i = unsafe { NSToolbarItem::initWithItemIdentifier(NSToolbarItem::alloc(mtm()), &ns(id)) };
    configure_tool(&i, label, symbol_name, tip, f);
    i
}

fn configure_tool(i: &NSToolbarItem, label: &str, symbol_name: &str, tip: &str, f: impl Fn() + 'static) {
    let h = Handler::simple(f);
    unsafe {
        i.setLabel(&ns(label));
        i.setPaletteLabel(&ns(label));
        i.setToolTip(Some(&ns(tip)));
        i.setImage(symbol(symbol_name, label).as_deref());
        i.setBordered(true);
        i.setAutovalidates(false);
        i.setTarget(Some(&h));
        i.setAction(Some(sel!(fire:)));
    }
    keep(i, &h);
}

/// A toolbar button with a menu: a click runs `f`, the arrow opens `menu`.
pub fn menu_tool(id: &str, label: &str, symbol_name: &str, tip: &str, menu: &NSMenu, f: impl Fn() + 'static) -> Retained<NSMenuToolbarItem> {
    let i = unsafe { NSMenuToolbarItem::initWithItemIdentifier(NSMenuToolbarItem::alloc(mtm()), &ns(id)) };
    configure_tool(&i, label, symbol_name, tip, f);
    unsafe {
        i.setMenu(menu);
        i.setShowsIndicator(true);
    }
    i
}

// ---------------------------------------------------------------------------
// a view that takes dropped links

pub struct DropIvars {
    dropped: RefCell<Option<DropFn>>,
}

fn dragged_text(info: &AnyObject) -> Vec<String> {
    let pasteboard: Retained<NSPasteboard> = unsafe { msg_send![info, draggingPasteboard] };
    let text = unsafe { pasteboard.stringForType(NSPasteboardTypeURL).or_else(|| pasteboard.stringForType(NSPasteboardTypeString)) };
    text.map(|t| t.to_string().split_whitespace().map(String::from).collect()).unwrap_or_default()
}

define_class!(
    /// A container that accepts links dragged from a browser.
    #[unsafe(super(NSView, NSResponder, NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "ODMDropView"]
    #[ivars = DropIvars]
    pub struct DropView;

    impl DropView {
        #[unsafe(method(draggingEntered:))]
        fn dragging_entered(&self, info: &AnyObject) -> NSDragOperation {
            if dragged_text(info).iter().any(|u| odm_ui::fmt::is_http(u)) {
                NSDragOperation::Copy
            } else {
                NSDragOperation::None
            }
        }

        #[unsafe(method(performDragOperation:))]
        fn perform_drag(&self, info: &AnyObject) -> bool {
            let links: Vec<String> = dragged_text(info).into_iter().filter(|u| odm_ui::fmt::is_http(u)).collect();
            let any = !links.is_empty();
            if any {
                if let Some(f) = &*self.ivars().dropped.borrow() {
                    f(links);
                }
            }
            any
        }
    }
);

impl DropView {
    pub fn new(dropped: impl Fn(Vec<String>) + 'static) -> Retained<Self> {
        let this = Self::alloc(mtm()).set_ivars(DropIvars { dropped: RefCell::new(Some(Box::new(dropped))) });
        let this: Retained<Self> = unsafe { msg_send![super(this), initWithFrame: NSRect::ZERO] };
        unsafe { this.registerForDraggedTypes(&NSArray::from_slice(&[NSPasteboardTypeURL, NSPasteboardTypeString])) };
        this
    }
}

/// A labeled row of a form: the label right-aligned in a fixed column.
pub fn row(title: &str, control: &NSView) -> Retained<NSStackView> {
    wide_row(130.0, title, control)
}

/// A labeled row with a label column of `label_width`.
pub fn wide_row(label_width: f64, title: &str, control: &NSView) -> Retained<NSStackView> {
    let l = label(title);
    unsafe { l.setAlignment(NSTextAlignment::Right) };
    width(&l, label_width);
    let r = hstack(10.0, &[&l, control]);
    unsafe {
        // controls that can grow (text fields, stretched stacks) fill the
        // row; the others keep their size
        let fill = control.trailingAnchor().constraintEqualToAnchor(&r.trailingAnchor());
        fill.setPriority(240.0);
        fill.setActive(true);
    }
    r
}

/// A row whose label sits at the top (next to a tall control).
pub fn tall_row(title: &str, control: &NSView) -> Retained<NSStackView> {
    let r = row(title, control);
    unsafe { r.setAlignment(NSLayoutAttribute::Top) };
    r
}

/// A bold heading for a group of rows.
pub fn heading(text: &str) -> Retained<NSTextField> {
    let l = label(text);
    set_font(&l, 13.0, true);
    l
}

/// A small spinning progress indicator.
pub fn spinner() -> Retained<NSProgressIndicator> {
    let p = NSProgressIndicator::new(mtm());
    unsafe {
        p.setStyle(NSProgressIndicatorStyle::Spinning);
        p.setControlSize(NSControlSize::Small);
        p.setDisplayedWhenStopped(false);
    }
    p
}

/// An on/off switch.
pub fn toggle(f: impl Fn() + 'static) -> Retained<NSSwitch> {
    let s = NSSwitch::new(mtm());
    on_action(&s, f);
    s
}

pub fn switch_on(s: &NSSwitch) -> bool {
    unsafe { s.state() == NSControlStateValueOn }
}

pub fn set_switch(s: &NSSwitch, on: bool) {
    unsafe { s.setState(if on { NSControlStateValueOn } else { NSControlStateValueOff }) };
}

pub fn red(l: &NSTextField) {
    unsafe { l.setTextColor(Some(&NSColor::systemRedColor())) };
}

/// A label that wraps and can be selected (for errors).
pub fn wrapping(text: &str) -> Retained<NSTextField> {
    let l = unsafe { NSTextField::wrappingLabelWithString(&ns(text), mtm()) };
    l.setContentCompressionResistancePriority_forOrientation(250.0, NSLayoutConstraintOrientation::Horizontal);
    l.setContentHuggingPriority_forOrientation(1.0, NSLayoutConstraintOrientation::Horizontal);
    l
}

/// A closure that runs `f` on the owner of a window's controls, if it still
/// exists (controls hold their owner weakly, so closing a window frees it).
pub fn bound<T: 'static>(weak: &std::rc::Weak<T>, f: impl Fn(&std::rc::Rc<T>) + Clone + 'static) -> impl Fn() + Clone + 'static {
    let weak = weak.clone();
    move || {
        if let Some(owner) = weak.upgrade() {
            f(&owner);
        }
    }
}

/// Let a stack take up the slack of the row or column it sits in.
pub fn stretch(s: &NSStackView, horizontal: bool) {
    let o = if horizontal { NSLayoutConstraintOrientation::Horizontal } else { NSLayoutConstraintOrientation::Vertical };
    unsafe { s.setHuggingPriority_forOrientation(1.0, o) };
    s.setContentHuggingPriority_forOrientation(1.0, o);
}

/// Let a view (a label, a table) take up the slack of its row.
pub fn stretch_view(v: &NSView) {
    v.setContentHuggingPriority_forOrientation(1.0, NSLayoutConstraintOrientation::Horizontal);
}

/// `over` centered on top of `base` (a note over an empty table).
pub fn overlay(base: &NSView, over: &NSView) -> Retained<NSView> {
    let container = NSView::new(mtm());
    fill(&container, base);
    unsafe {
        over.setTranslatesAutoresizingMaskIntoConstraints(false);
        container.addSubview(over);
        over.centerXAnchor().constraintEqualToAnchor(&container.centerXAnchor()).setActive(true);
        over.centerYAnchor().constraintEqualToAnchor(&container.centerYAnchor()).setActive(true);
    }
    container.setContentHuggingPriority_forOrientation(1.0, NSLayoutConstraintOrientation::Vertical);
    container
}
