//! The menu bar item (`Tray`): one `NSStatusItem` with a template icon and an `NSMenu`.
//!
//! macOS opens the menu on a click, so `TrayMenu::default_action` has no click of its own to
//! bind (the Windows and Linux trays run it on a left click). The menu already holds the app's
//! entries, so nothing extra is added for it.

use std::cell::RefCell;
use std::panic::{AssertUnwindSafe, catch_unwind};

use laqta_domain::Action;
use laqta_ports::{ActionCallback, PortError, Tray, TrayMenu};
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send, sel};
use objc2_app_kit::{NSApplication, NSImage, NSMenu, NSMenuItem, NSStatusBar, NSStatusItem, NSVariableStatusItemLength};
use objc2_foundation::{NSObject, NSObjectProtocol, NSString, ns_string};

use crate::main_thread::on_main;

#[derive(Debug, Default)]
pub struct MacTray;

impl Tray for MacTray {
    fn show(&self, menu: TrayMenu, on_select: ActionCallback) -> Result<(), PortError> {
        on_main(move |mtm| show_on_main(mtm, menu, on_select))
    }
}

/// What a menu item's tag means: `actions[tag]` is the action to run, `callback` runs it.
struct Dispatch {
    actions: Vec<Action>,
    callback: Option<ActionCallback>,
}

struct TargetIvars {
    // Only touched on the main thread; never borrowed while the callback runs.
    dispatch: RefCell<Dispatch>,
}

define_class!(
    // SAFETY: NSObject has no subclassing requirements and `MenuTarget` does not implement Drop.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[ivars = TargetIvars]
    struct MenuTarget;

    impl MenuTarget {
        // SAFETY: AppKit calls a menu item's action with the item as the only argument.
        #[unsafe(method(select:))]
        fn select(&self, sender: &AnyObject) {
            let Some(item) = sender.downcast_ref::<NSMenuItem>() else { return };
            let tag = item.tag();
            let picked = {
                let dispatch = self.ivars().dispatch.borrow();
                let action = usize::try_from(tag).ok().and_then(|i| dispatch.actions.get(i).copied());
                action.zip(dispatch.callback.clone())
            };
            // The borrow is gone, so the callback may call `show` again.
            if let Some((action, callback)) = picked
                && catch_unwind(AssertUnwindSafe(|| callback(action))).is_err()
            {
                tracing::error!(?action, "the menu bar item's callback panicked");
            }
        }
    }

    // SAFETY: `NSObjectProtocol` has no safety requirements.
    unsafe impl NSObjectProtocol for MenuTarget {}
);

impl MenuTarget {
    fn new(mtm: MainThreadMarker) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(TargetIvars { dispatch: RefCell::new(Dispatch { actions: Vec::new(), callback: None }) });
        // SAFETY: NSObject's `init` takes no arguments and returns the initialised object.
        unsafe { msg_send![super(this), init] }
    }
}

/// The status item and what keeps its menu working. `NSMenuItem` targets are weak references, so
/// `target` must outlive the items.
struct TrayState {
    item: Retained<NSStatusItem>,
    target: Retained<MenuTarget>,
}

thread_local! {
    // AppKit objects live on the main thread only; `show_on_main` is the sole user.
    static STATE: RefCell<Option<TrayState>> = const { RefCell::new(None) };
}

fn show_on_main(mtm: MainThreadMarker, menu: TrayMenu, on_select: ActionCallback) -> Result<(), PortError> {
    STATE.with(|cell| {
        let mut state = cell.borrow_mut();
        let state = state.get_or_insert_with(|| create(mtm));

        // The target is set up before the items so a click can never see the old table.
        {
            let mut dispatch = state.target.ivars().dispatch.borrow_mut();
            dispatch.actions = menu.items.iter().map(|i| i.action).collect();
            dispatch.callback = Some(on_select);
        }

        let ns_menu = NSMenu::initWithTitle(NSMenu::alloc(mtm), ns_string!(""));
        for (index, entry) in menu.items.iter().enumerate() {
            let title = NSString::from_str(&entry.label);
            // SAFETY: `select:` is implemented by `MenuTarget`, which `setTarget` is given below.
            let item = unsafe {
                NSMenuItem::initWithTitle_action_keyEquivalent(NSMenuItem::alloc(mtm), &title, Some(sel!(select:)), ns_string!(""))
            };
            // The index is the key into `Dispatch::actions`; a menu never holds near `isize::MAX` items.
            item.setTag(isize::try_from(index).unwrap_or(isize::MAX));
            // SAFETY: the target is a `MenuTarget`, kept alive by `TrayState` (the item holds it weakly).
            unsafe { item.setTarget(Some(&state.target)) };
            ns_menu.addItem(&item);
        }
        // The status item retains the new menu and releases the previous one.
        state.item.setMenu(Some(&ns_menu));

        if let Some(button) = state.item.button(mtm) {
            button.setToolTip(Some(&NSString::from_str(&menu.tooltip)));
        }
        Ok(())
    })
}

/// Makes the status item with its icon. Done once; later `show` calls only swap the menu.
fn create(mtm: MainThreadMarker) -> TrayState {
    // A status item needs the shared application (and its window server connection) to exist.
    let _ = NSApplication::sharedApplication(mtm);
    let item = NSStatusBar::systemStatusBar().statusItemWithLength(NSVariableStatusItemLength);
    if let Some(button) = item.button(mtm) {
        // An SF Symbol (macOS 11+) as a template image follows light and dark menu bars.
        match NSImage::imageWithSystemSymbolName_accessibilityDescription(ns_string!("camera.viewfinder"), Some(ns_string!("Laqta"))) {
            Some(image) => {
                image.setTemplate(true);
                button.setImage(Some(&image));
            }
            None => button.setTitle(ns_string!("Laqta")),
        }
    }
    TrayState { item, target: MenuTarget::new(mtm) }
}
