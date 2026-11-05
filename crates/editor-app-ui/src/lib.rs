slint::include_modules!();

use slint::ComponentHandle;

#[derive(Clone, Default)]
pub struct DocumentFocus(slint::private_unstable_api::re_exports::ItemWeak);

fn take_focus(window: &slint::Window) -> DocumentFocus {
    let inner = slint::private_unstable_api::re_exports::WindowInner::from_pub(window);
    let focus = inner.focus_item.borrow().clone();
    if let Some(item) = focus.upgrade() {
        inner.set_focus_item(
            &item,
            false,
            slint::private_unstable_api::re_exports::FocusReason::WindowActivation,
        );
    }
    DocumentFocus(focus)
}

fn restore_focus(window: &slint::Window, focus: &DocumentFocus) -> bool {
    let Some(item) = focus.0.upgrade().filter(|item| item.is_visible()) else {
        return false;
    };
    slint::private_unstable_api::re_exports::WindowInner::from_pub(window).set_focus_item(
        &item,
        true,
        slint::private_unstable_api::re_exports::FocusReason::Programmatic,
    );
    true
}

fn document_is_current(view: &AppWindow) -> bool {
    let root = slint::private_unstable_api::re_exports::VRc::into_dyn(view.clone_strong().into());
    slint::private_unstable_api::re_exports::WindowInner::from_pub(view.window())
        .try_component()
        .is_some_and(|current| {
            slint::private_unstable_api::re_exports::VRc::ptr_eq(&current, &root)
        })
}

pub fn capture_document_focus(view: &AppWindow) -> Result<DocumentFocus, String> {
    if !document_is_current(view) {
        return Ok(DocumentFocus::default());
    }
    let inner = slint::private_unstable_api::re_exports::WindowInner::from_pub(view.window());
    if !inner.active_popups().is_empty() {
        return Err("Close the menu or popup before switching documents".into());
    }
    Ok(take_focus(view.window()))
}

pub fn restore_document_focus(view: &AppWindow, focus: &DocumentFocus) -> Result<(), String> {
    if !document_is_current(view) {
        return Err("The document view is not active".into());
    }
    if !restore_focus(view.window(), focus) {
        view.invoke_focus();
    }
    Ok(())
}

pub fn prepare_document_retirement(view: &AppWindow) {
    if document_is_current(view) {
        slint::private_unstable_api::re_exports::WindowInner::from_pub(view.window())
            .close_all_popups();
        take_focus(view.window());
    }
}

pub fn new_document_view(window: &slint::Window) -> Result<AppWindow, slint::PlatformError> {
    let inner = slint::private_unstable_api::re_exports::WindowInner::from_pub(window);
    if !inner.active_popups().is_empty() {
        return Err("Close the menu or popup before preparing another document".into());
    }
    let previous = inner
        .try_component()
        .ok_or("The workspace display root is unavailable")?;
    let focus = take_focus(window);
    let result = AppWindow::new_with_existing_window(window);
    take_focus(window);
    inner.set_component(&previous);
    restore_focus(window, &focus);
    result
}

fn activate_view(
    window: &slint::Window,
    root: slint::private_unstable_api::re_exports::ItemTreeRc,
) -> Result<(), String> {
    let inner = slint::private_unstable_api::re_exports::WindowInner::from_pub(window);
    if inner.try_component().is_some_and(|current| {
        slint::private_unstable_api::re_exports::VRc::ptr_eq(&current, &root)
    }) {
        return Ok(());
    }
    if !inner.active_popups().is_empty() {
        return Err("Close the menu or popup before switching documents".into());
    }
    take_focus(window);
    inner.set_component(&root);
    if window.is_visible() {
        window.show().map_err(|error| error.to_string())?;
    }
    window.request_redraw();
    Ok(())
}

pub fn activate_document_view(view: &AppWindow) -> Result<(), String> {
    let root = slint::private_unstable_api::re_exports::VRc::into_dyn(view.clone_strong().into());
    activate_view(view.window(), root)
}

pub fn activate_workspace_view(view: &WorkspaceWindow) -> Result<(), String> {
    let root = slint::private_unstable_api::re_exports::VRc::into_dyn(view.clone_strong().into());
    activate_view(view.window(), root)?;
    let inner = slint::private_unstable_api::re_exports::WindowInner::from_pub(view.window());
    let has_focus = inner.focus_item.borrow().upgrade().is_some();
    if !has_focus {
        inner.focus_next_item();
    }
    Ok(())
}

pub fn activate_workspace_after_retirement(view: &WorkspaceWindow) -> Result<(), String> {
    slint::private_unstable_api::re_exports::WindowInner::from_pub(view.window())
        .close_all_popups();
    activate_workspace_view(view)
}
