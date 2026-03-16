use std::cell::RefCell;
use std::rc::Rc;

use gtk4 as gtk;
use gtk::prelude::*;
use gtk::glib;
use libadwaita as adw;
use adw::prelude::*;

use crate::pane::{self, PaneCallbacks};

// ---------------------------------------------------------------------------
// State
// ---------------------------------------------------------------------------

struct Workspace {
    id: String,
    name: String,
    /// The root widget in the content stack for this workspace.
    root: gtk::Widget,
    /// The sidebar row widget.
    sidebar_row: gtk::ListBoxRow,
    /// Name label in sidebar row.
    name_label: gtk::Label,
    /// Notification dot in the sidebar row.
    notify_dot: gtk::Label,
    /// Notification message label in the sidebar row.
    notify_label: gtk::Label,
    /// Whether this workspace has unread notifications.
    unread: bool,
}

struct AppState {
    workspaces: Vec<Workspace>,
    active_idx: usize,
    next_number: usize,
    stack: gtk::Stack,
    sidebar_list: gtk::ListBox,
    paned: gtk::Paned,
}

impl AppState {
    fn active_workspace(&self) -> Option<&Workspace> {
        self.workspaces.get(self.active_idx)
    }
}

type State = Rc<RefCell<AppState>>;

// ---------------------------------------------------------------------------
// CSS
// ---------------------------------------------------------------------------

const CSS: &str = r#"
.cmux-sidebar {
    background-color: rgba(25, 25, 25, 1);
}
.cmux-sidebar-row-box {
    padding: 6px 12px;
    border-radius: 6px;
    margin: 2px 6px;
}
.cmux-ws-name {
    color: rgba(255, 255, 255, 0.7);
    font-size: 13px;
}
row:selected .cmux-ws-name {
    color: white;
}
.cmux-notify-dot {
    color: #0091FF;
    font-size: 10px;
    margin-right: 6px;
}
.cmux-notify-dot-hidden {
    color: transparent;
    font-size: 10px;
    margin-right: 6px;
}
.cmux-notify-msg {
    color: rgba(255, 255, 255, 0.35);
    font-size: 11px;
}
.cmux-notify-msg-unread {
    color: rgba(0, 145, 255, 0.8);
    font-size: 11px;
}
.cmux-sidebar-title {
    color: rgba(255, 255, 255, 0.5);
    font-size: 11px;
    font-weight: 600;
    letter-spacing: 1px;
}
.cmux-sidebar-btn {
    background: rgba(255, 255, 255, 0.08);
    color: rgba(255, 255, 255, 0.7);
    border: none;
    border-radius: 6px;
    padding: 6px 12px;
    min-height: 0;
}
.cmux-sidebar-btn:hover {
    background: rgba(255, 255, 255, 0.14);
    color: white;
}
.cmux-content {
    background-color: rgba(23, 23, 23, 1);
}
"#;

// ---------------------------------------------------------------------------
// Window construction
// ---------------------------------------------------------------------------

pub fn build_window(app: &adw::Application) {
    // Load CSS
    let provider = gtk::CssProvider::new();
    let all_css = format!("{CSS}\n{}", pane::PANE_CSS);
    provider.load_from_data(&all_css);
    gtk::style_context_add_provider_for_display(
        &gtk::gdk::Display::default().expect("display"),
        &provider,
        gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
    );

    adw::StyleManager::default().set_color_scheme(adw::ColorScheme::ForceDark);

    let window = adw::ApplicationWindow::builder()
        .application(app)
        .title("cmux")
        .default_width(1400)
        .default_height(900)
        .build();

    let header = adw::HeaderBar::new();
    header.set_title_widget(Some(&gtk::Label::builder().label("cmux").build()));

    let stack = gtk::Stack::new();
    stack.set_transition_type(gtk::StackTransitionType::None);
    stack.set_hexpand(true);
    stack.set_vexpand(true);
    stack.add_css_class("cmux-content");

    let sidebar_list = gtk::ListBox::new();
    sidebar_list.set_selection_mode(gtk::SelectionMode::Single);
    sidebar_list.add_css_class("navigation-sidebar");

    let sidebar_scroll = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vscrollbar_policy(gtk::PolicyType::Automatic)
        .vexpand(true)
        .child(&sidebar_list)
        .build();

    let sidebar_title = gtk::Label::builder()
        .label("WORKSPACES")
        .xalign(0.0)
        .margin_start(12)
        .margin_top(8)
        .margin_bottom(4)
        .build();
    sidebar_title.add_css_class("cmux-sidebar-title");

    let new_ws_btn = gtk::Button::builder().label("New Workspace").build();
    new_ws_btn.add_css_class("cmux-sidebar-btn");

    let sidebar = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(4)
        .width_request(220)
        .build();
    sidebar.add_css_class("cmux-sidebar");
    sidebar.append(&sidebar_title);
    sidebar.append(&sidebar_scroll);
    sidebar.append(&new_ws_btn);

    let main_paned = gtk::Paned::builder()
        .orientation(gtk::Orientation::Horizontal)
        .position(220)
        .shrink_start_child(false)
        .shrink_end_child(false)
        .start_child(&sidebar)
        .end_child(&stack)
        .build();

    let vbox = gtk::Box::new(gtk::Orientation::Vertical, 0);
    vbox.append(&header);
    vbox.append(&main_paned);
    window.set_content(Some(&vbox));

    let state: State = Rc::new(RefCell::new(AppState {
        workspaces: Vec::new(),
        active_idx: 0,
        next_number: 1,
        stack: stack.clone(),
        sidebar_list: sidebar_list.clone(),
        paned: main_paned.clone(),
    }));

    register_actions(&window, &state);
    install_key_capture(&window, &state);

    {
        let state = state.clone();
        sidebar_list.connect_row_selected(move |_, row| {
            if let Some(row) = row {
                let idx = row.index() as usize;
                switch_workspace(&state, idx);
            }
        });
    }

    {
        let state = state.clone();
        new_ws_btn.connect_clicked(move |_| {
            add_workspace(&state, None);
        });
    }

    add_workspace(&state, None);
    window.present();
}

// ---------------------------------------------------------------------------
// Actions
// ---------------------------------------------------------------------------

fn register_actions(window: &adw::ApplicationWindow, state: &State) {
    let action_defs: &[&str] = &[
        "new-workspace",
        "close-workspace",
        "toggle-sidebar",
        "next-workspace",
        "prev-workspace",
    ];

    for name in action_defs {
        let action = gtk::gio::SimpleAction::new(name, None);
        let state = state.clone();
        let handler_name = name.to_string();
        action.connect_activate(move |_, _| {
            match handler_name.as_str() {
                "new-workspace" => add_workspace(&state, None),
                "close-workspace" => close_workspace(&state),
                "toggle-sidebar" => toggle_sidebar(&state),
                "next-workspace" => cycle_workspace(&state, 1),
                "prev-workspace" => cycle_workspace(&state, -1),
                _ => {}
            }
        });
        window.add_action(&action);
    }
}

/// Intercept keyboard shortcuts in the CAPTURE phase so VTE doesn't eat them.
fn install_key_capture(window: &adw::ApplicationWindow, state: &State) {
    use gtk::gdk;

    let key_controller = gtk::EventControllerKey::new();
    key_controller.set_propagation_phase(gtk::PropagationPhase::Capture);

    let state = state.clone();
    key_controller.connect_key_pressed(move |_, keyval, _keycode, modifier| {
        let ctrl = modifier.contains(gdk::ModifierType::CONTROL_MASK);
        let shift = modifier.contains(gdk::ModifierType::SHIFT_MASK);

        // Strip lock keys (CapsLock, NumLock) from comparison
        let matched = match (ctrl, shift, keyval) {
            // Ctrl+Shift+N → new workspace
            (true, true, gdk::Key::N | gdk::Key::n) => {
                add_workspace(&state, None);
                true
            }
            // Ctrl+Shift+W → close workspace
            (true, true, gdk::Key::W | gdk::Key::w) => {
                close_workspace(&state);
                true
            }
            // Ctrl+Shift+D → split down
            (true, true, gdk::Key::D | gdk::Key::d) => {
                split_focused_pane(&state, gtk::Orientation::Vertical);
                true
            }
            // Ctrl+Shift+T → new terminal tab in focused pane
            (true, true, gdk::Key::T | gdk::Key::t) => {
                add_tab_to_focused_pane(&state, false);
                true
            }
            // Ctrl+D → split right
            (true, false, gdk::Key::d) => {
                split_focused_pane(&state, gtk::Orientation::Horizontal);
                true
            }
            // Ctrl+W → close focused tab/pane
            (true, false, gdk::Key::w) => {
                close_focused_tab(&state);
                true
            }
            // Ctrl+B → toggle sidebar
            (true, false, gdk::Key::b) => {
                toggle_sidebar(&state);
                true
            }
            // Ctrl+T → new terminal tab
            (true, false, gdk::Key::t) => {
                add_tab_to_focused_pane(&state, false);
                true
            }
            // Ctrl+PageDown → next workspace
            (true, false, gdk::Key::Page_Down) => {
                cycle_workspace(&state, 1);
                true
            }
            // Ctrl+PageUp → prev workspace
            (true, false, gdk::Key::Page_Up) => {
                cycle_workspace(&state, -1);
                true
            }
            // Ctrl+1-9 → switch to workspace by index
            (true, false, key) => {
                let digit = match key {
                    gdk::Key::_1 => Some(0usize),
                    gdk::Key::_2 => Some(1),
                    gdk::Key::_3 => Some(2),
                    gdk::Key::_4 => Some(3),
                    gdk::Key::_5 => Some(4),
                    gdk::Key::_6 => Some(5),
                    gdk::Key::_7 => Some(6),
                    gdk::Key::_8 => Some(7),
                    gdk::Key::_9 => {
                        // Ctrl+9 always goes to last workspace
                        let s = state.borrow();
                        if s.workspaces.is_empty() { None }
                        else { Some(s.workspaces.len() - 1) }
                    }
                    _ => None,
                };
                if let Some(idx) = digit {
                    let row_and_list = {
                        let s = state.borrow();
                        s.workspaces.get(idx).map(|ws| (ws.sidebar_row.clone(), s.sidebar_list.clone()))
                    };
                    switch_workspace(&state, idx);
                    if let Some((row, list)) = row_and_list {
                        list.select_row(Some(&row));
                    }
                    true
                } else {
                    false
                }
            }
            _ => false,
        };

        if matched {
            glib::Propagation::Stop
        } else {
            glib::Propagation::Proceed
        }
    });

    window.add_controller(key_controller);
}

// ---------------------------------------------------------------------------
// Sidebar row
// ---------------------------------------------------------------------------

fn build_sidebar_row(name: &str) -> (gtk::ListBoxRow, gtk::Label, gtk::Label, gtk::Label) {
    let notify_dot = gtk::Label::builder().label("\u{25CF}").build();
    notify_dot.add_css_class("cmux-notify-dot-hidden");

    let name_label = gtk::Label::builder()
        .label(name)
        .xalign(0.0)
        .hexpand(true)
        .ellipsize(gtk::pango::EllipsizeMode::End)
        .build();
    name_label.add_css_class("cmux-ws-name");

    let top_row = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    top_row.append(&notify_dot);
    top_row.append(&name_label);

    let notify_label = gtk::Label::builder()
        .xalign(0.0)
        .ellipsize(gtk::pango::EllipsizeMode::End)
        .visible(false)
        .margin_start(16)
        .build();
    notify_label.add_css_class("cmux-notify-msg");

    let vbox = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(2)
        .build();
    vbox.add_css_class("cmux-sidebar-row-box");
    vbox.append(&top_row);
    vbox.append(&notify_label);

    let row = gtk::ListBoxRow::new();
    row.set_child(Some(&vbox));

    (row, name_label, notify_dot, notify_label)
}

// ---------------------------------------------------------------------------
// Workspace management
// ---------------------------------------------------------------------------

fn add_workspace(state: &State, working_directory: Option<&str>) {
    let mut s = state.borrow_mut();
    let number = s.next_number;
    s.next_number += 1;

    let id = uuid::Uuid::new_v4().to_string();
    let name = format!("Terminal {number}");
    let stack_name = format!("ws-{id}");

    // Create the initial pane for this workspace
    let pane_widget = create_pane_for_workspace(state, &id, working_directory);
    let root: gtk::Widget = pane_widget.upcast();

    s.stack.add_named(&root, Some(&stack_name));

    let (row, name_label, notify_dot, notify_label) = build_sidebar_row(&name);
    s.sidebar_list.append(&row);

    let ws = Workspace {
        id,
        name,
        root,
        sidebar_row: row.clone(),
        name_label,
        notify_dot,
        notify_label,
        unread: false,
    };

    s.workspaces.push(ws);
    let new_idx = s.workspaces.len() - 1;
    s.active_idx = new_idx;
    s.stack.set_visible_child_name(&stack_name);

    let sidebar_list = s.sidebar_list.clone();
    drop(s);

    sidebar_list.select_row(Some(&row));
}

/// Create a PaneWidget wired up with callbacks for a specific workspace.
fn create_pane_for_workspace(
    state: &State,
    ws_id: &str,
    working_directory: Option<&str>,
) -> gtk::Box {
    let state_for_split = state.clone();
    let state_for_close = state.clone();
    let state_for_bell = state.clone();
    let state_for_empty = state.clone();
    let ws_id_split = ws_id.to_string();
    let ws_id_close = ws_id.to_string();
    let ws_id_bell = ws_id.to_string();
    let ws_id_empty = ws_id.to_string();

    let callbacks = Rc::new(PaneCallbacks {
        on_split: Box::new(move |pane_widget, orientation| {
            split_pane(&state_for_split, &ws_id_split, pane_widget, orientation);
        }),
        on_close_pane: Box::new(move |pane_widget| {
            remove_pane(&state_for_close, &ws_id_close, pane_widget);
        }),
        on_bell: Box::new(move || {
            mark_workspace_unread(&state_for_bell, &ws_id_bell);
        }),
        on_empty: Box::new(move |pane_widget| {
            remove_pane(&state_for_empty, &ws_id_empty, pane_widget);
        }),
    });

    pane::create_pane(callbacks, working_directory)
}

fn close_workspace(state: &State) {
    let id = {
        let s = state.borrow();
        s.active_workspace().map(|w| w.id.clone())
    };
    if let Some(id) = id {
        close_workspace_by_id(state, &id);
    }
}

fn close_workspace_by_id(state: &State, id: &str) {
    let mut s = state.borrow_mut();
    let Some(idx) = s.workspaces.iter().position(|w| w.id == id) else {
        return;
    };

    let ws = s.workspaces.remove(idx);
    s.stack.remove(&ws.root);
    s.sidebar_list.remove(&ws.sidebar_row);

    if s.workspaces.is_empty() {
        if let Some(root) = s.stack.root() {
            if let Some(window) = root.downcast_ref::<gtk::Window>() {
                drop(s);
                window.close();
            }
        }
        return;
    }

    let new_idx = idx.min(s.workspaces.len() - 1);
    s.active_idx = new_idx;

    let stack_name = format!("ws-{}", s.workspaces[new_idx].id);
    s.stack.set_visible_child_name(&stack_name);

    let row = s.workspaces[new_idx].sidebar_row.clone();
    let sidebar_list = s.sidebar_list.clone();
    drop(s);

    sidebar_list.select_row(Some(&row));
}

fn switch_workspace(state: &State, idx: usize) {
    let mut s = state.borrow_mut();
    if idx >= s.workspaces.len() || idx == s.active_idx {
        return;
    }
    s.active_idx = idx;
    let stack_name = format!("ws-{}", s.workspaces[idx].id);
    s.stack.set_visible_child_name(&stack_name);

    // Clear unread
    let ws = &mut s.workspaces[idx];
    if ws.unread {
        ws.unread = false;
        ws.notify_dot.remove_css_class("cmux-notify-dot");
        ws.notify_dot.add_css_class("cmux-notify-dot-hidden");
        ws.notify_label.remove_css_class("cmux-notify-msg-unread");
        ws.notify_label.add_css_class("cmux-notify-msg");
    }
}

fn cycle_workspace(state: &State, direction: i32) {
    let (new_idx, row, sidebar_list) = {
        let s = state.borrow();
        let len = s.workspaces.len();
        if len <= 1 { return; }
        let new_idx = ((s.active_idx as i32 + direction).rem_euclid(len as i32)) as usize;
        (new_idx, s.workspaces[new_idx].sidebar_row.clone(), s.sidebar_list.clone())
    };
    switch_workspace(state, new_idx);
    sidebar_list.select_row(Some(&row));
}

fn toggle_sidebar(state: &State) {
    let s = state.borrow();
    if let Some(sidebar) = s.paned.start_child() {
        sidebar.set_visible(!sidebar.is_visible());
    }
}

// ---------------------------------------------------------------------------
// Split / close pane operations
// ---------------------------------------------------------------------------

fn split_pane(
    state: &State,
    ws_id: &str,
    pane_widget: &gtk::Widget,
    orientation: gtk::Orientation,
) {
    let new_pane = create_pane_for_workspace(state, ws_id, None);

    let parent = pane_widget.parent();

    let new_paned = gtk::Paned::builder()
        .orientation(orientation)
        .hexpand(true)
        .vexpand(true)
        .build();

    if let Some(parent) = parent {
        if let Some(paned_parent) = parent.downcast_ref::<gtk::Paned>() {
            let is_start = paned_parent.start_child()
                .map(|c| c == *pane_widget)
                .unwrap_or(false);
            if is_start {
                paned_parent.set_start_child(Some(&new_paned));
            } else {
                paned_parent.set_end_child(Some(&new_paned));
            }
        } else if let Some(stack) = parent.downcast_ref::<gtk::Stack>() {
            let page_name = format!("ws-{ws_id}");
            stack.remove(pane_widget);
            stack.add_named(&new_paned, Some(&page_name));
            stack.set_visible_child_name(&page_name);
            // Update root reference
            let mut s = state.borrow_mut();
            if let Some(ws) = s.workspaces.iter_mut().find(|w| w.id == ws_id) {
                ws.root = new_paned.clone().upcast();
            }
        }
    }

    new_paned.set_start_child(Some(pane_widget));
    new_paned.set_end_child(Some(&new_pane));

    // 50% split after layout
    {
        let np = new_paned.clone();
        glib::idle_add_local_once(move || {
            let alloc = np.allocation();
            let size = if orientation == gtk::Orientation::Horizontal {
                alloc.width()
            } else {
                alloc.height()
            };
            if size > 0 { np.set_position(size / 2); }
        });
    }
}

fn remove_pane(state: &State, ws_id: &str, pane_widget: &gtk::Widget) {
    let parent = pane_widget.parent();

    let Some(parent) = parent else { return; };

    if let Some(paned) = parent.downcast_ref::<gtk::Paned>() {
        // Find sibling
        let sibling = if paned.start_child().map(|c| c == *pane_widget).unwrap_or(false) {
            paned.end_child()
        } else {
            paned.start_child()
        };

        if let Some(sibling) = sibling {
            paned.set_start_child(gtk::Widget::NONE);
            paned.set_end_child(gtk::Widget::NONE);

            if let Some(grandparent) = paned.parent() {
                if let Some(gp_paned) = grandparent.downcast_ref::<gtk::Paned>() {
                    let is_start = gp_paned.start_child()
                        .map(|c| c == paned.clone().upcast::<gtk::Widget>())
                        .unwrap_or(false);
                    if is_start {
                        gp_paned.set_start_child(Some(&sibling));
                    } else {
                        gp_paned.set_end_child(Some(&sibling));
                    }
                } else if let Some(stack) = grandparent.downcast_ref::<gtk::Stack>() {
                    let page_name = format!("ws-{ws_id}");
                    stack.remove(paned);
                    stack.add_named(&sibling, Some(&page_name));
                    stack.set_visible_child_name(&page_name);
                    let mut s = state.borrow_mut();
                    if let Some(ws) = s.workspaces.iter_mut().find(|w| w.id == ws_id) {
                        ws.root = sibling.clone();
                    }
                }
            }
        }
    } else if parent.downcast_ref::<gtk::Stack>().is_some() {
        // This is the only pane in the workspace — close the workspace
        close_workspace_by_id(state, ws_id);
    }
}

/// Find the focused pane widget (a gtk::Box with class cmux-pane-toolbar child)
/// by walking up from the currently focused widget.
fn find_focused_pane(state: &State) -> Option<(String, gtk::Widget)> {
    let (ws_id, root, stack) = {
        let s = state.borrow();
        let ws = s.active_workspace()?;
        (ws.id.clone(), ws.root.clone(), s.stack.clone())
    };

    // Get the window's focus widget and walk up to find a pane Box
    let window = stack.root()?.downcast::<gtk::Window>().ok()?;
    let focus = gtk::prelude::GtkWindowExt::focus(&window)?;

    let mut widget: Option<gtk::Widget> = Some(focus);
    while let Some(w) = widget {
        if let Some(bx) = w.downcast_ref::<gtk::Box>() {
            let mut child = bx.first_child();
            while let Some(c) = child {
                if c.has_css_class("cmux-pane-toolbar") {
                    return Some((ws_id, w));
                }
                child = c.next_sibling();
            }
        }
        widget = w.parent();
    }

    Some((ws_id, root))
}

/// Find the GtkNotebook inside a pane widget.
fn find_notebook_in_pane(pane_widget: &gtk::Widget) -> Option<gtk::Notebook> {
    if let Some(bx) = pane_widget.downcast_ref::<gtk::Box>() {
        let mut child = bx.first_child();
        while let Some(c) = child {
            if let Ok(nb) = c.clone().downcast::<gtk::Notebook>() {
                return Some(nb);
            }
            child = c.next_sibling();
        }
    }
    None
}

fn split_focused_pane(state: &State, orientation: gtk::Orientation) {
    if let Some((ws_id, pane_widget)) = find_focused_pane(state) {
        split_pane(state, &ws_id, &pane_widget, orientation);
    }
}

fn close_focused_tab(state: &State) {
    if let Some((ws_id, pane_widget)) = find_focused_pane(state) {
        if let Some(notebook) = find_notebook_in_pane(&pane_widget) {
            if notebook.n_pages() > 1 {
                // Close current tab
                if let Some(page_num) = notebook.current_page() {
                    notebook.remove_page(Some(page_num));
                }
            } else {
                // Only one tab — close the whole pane
                remove_pane(state, &ws_id, &pane_widget);
            }
        }
    }
}

fn add_tab_to_focused_pane(state: &State, browser: bool) {
    if let Some((ws_id, pane_widget)) = find_focused_pane(state) {
        if let Some(notebook) = find_notebook_in_pane(&pane_widget) {
            if browser {
                pane::add_browser_tab(&notebook);
            } else {
                let callbacks = make_pane_callbacks(state, &ws_id);
                pane::add_terminal_tab(&notebook, None, &callbacks);
            }
        }
    }
}

fn make_pane_callbacks(state: &State, ws_id: &str) -> Rc<PaneCallbacks> {
    let s1 = state.clone();
    let s2 = state.clone();
    let s3 = state.clone();
    let s4 = state.clone();
    let id1 = ws_id.to_string();
    let id2 = ws_id.to_string();
    let id3 = ws_id.to_string();
    let id4 = ws_id.to_string();

    Rc::new(PaneCallbacks {
        on_split: Box::new(move |pw, o| split_pane(&s1, &id1, pw, o)),
        on_close_pane: Box::new(move |pw| remove_pane(&s2, &id2, pw)),
        on_bell: Box::new(move || mark_workspace_unread(&s3, &id3)),
        on_empty: Box::new(move |pw| remove_pane(&s4, &id4, pw)),
    })
}

fn mark_workspace_unread(state: &State, ws_id: &str) {
    let mut s = state.borrow_mut();
    let active_idx = s.active_idx;
    if let Some((idx, ws)) = s.workspaces.iter_mut().enumerate().find(|(_, w)| w.id == ws_id) {
        if idx != active_idx {
            ws.unread = true;
            ws.notify_dot.remove_css_class("cmux-notify-dot-hidden");
            ws.notify_dot.add_css_class("cmux-notify-dot");
            ws.notify_label.set_label("Process needs attention");
            ws.notify_label.remove_css_class("cmux-notify-msg");
            ws.notify_label.add_css_class("cmux-notify-msg-unread");
            ws.notify_label.set_visible(true);
        }
    }
}
