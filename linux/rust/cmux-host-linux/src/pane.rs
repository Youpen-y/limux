//! PaneWidget: a tabbed container with a toolbar.
//!
//! Each pane has:
//! - A toolbar at top: [+ Terminal] [+ Browser] [Split ↔] [Split ↕] [× Close]
//! - A GtkNotebook with tabs (each tab is a terminal or browser)
//!
//! This is the leaf node in the split tree. Splits wrap PaneWidgets in GtkPaned.

use std::cell::RefCell;
use std::rc::Rc;

use gtk4 as gtk;
use gtk::prelude::*;
use gtk::glib;

use vte4::TerminalExt;

use crate::terminal;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// Callback for pane actions that affect the workspace tree.
pub struct PaneCallbacks {
    /// Called when this pane wants to split. Args: (pane_widget, orientation)
    pub on_split: Box<dyn Fn(&gtk::Widget, gtk::Orientation)>,
    /// Called when this pane should be closed entirely.
    pub on_close_pane: Box<dyn Fn(&gtk::Widget)>,
    /// Called when a bell rings in a terminal within this pane.
    pub on_bell: Box<dyn Fn()>,
    /// Called when all tabs are closed (pane should be removed).
    pub on_empty: Box<dyn Fn(&gtk::Widget)>,
}

// ---------------------------------------------------------------------------
// CSS for pane toolbar
// ---------------------------------------------------------------------------

pub const PANE_CSS: &str = r#"
.cmux-pane-toolbar {
    background-color: rgba(30, 30, 30, 1);
    border-bottom: 1px solid rgba(255, 255, 255, 0.08);
    padding: 2px 4px;
    min-height: 28px;
}
.cmux-pane-toolbar button {
    background: none;
    border: none;
    border-radius: 4px;
    padding: 2px 6px;
    min-height: 0;
    min-width: 0;
    color: rgba(255, 255, 255, 0.5);
    font-size: 12px;
}
.cmux-pane-toolbar button:hover {
    background: rgba(255, 255, 255, 0.1);
    color: rgba(255, 255, 255, 0.85);
}
notebook > header {
    background-color: rgba(30, 30, 30, 1);
    border-bottom: 1px solid rgba(255, 255, 255, 0.06);
}
notebook > header tab {
    background: none;
    color: rgba(255, 255, 255, 0.5);
    padding: 4px 8px;
    min-height: 0;
    border-radius: 4px 4px 0 0;
}
notebook > header tab:checked {
    background: rgba(255, 255, 255, 0.08);
    color: white;
}
notebook > header tab button {
    background: none;
    border: none;
    padding: 0;
    min-height: 0;
    min-width: 0;
    color: rgba(255, 255, 255, 0.3);
}
notebook > header tab button:hover {
    color: rgba(255, 255, 255, 0.8);
}
"#;

// ---------------------------------------------------------------------------
// PaneWidget builder
// ---------------------------------------------------------------------------

/// Create a new pane widget with toolbar + notebook.
/// Returns the outermost GtkBox widget.
pub fn create_pane(
    callbacks: Rc<PaneCallbacks>,
    working_directory: Option<&str>,
) -> gtk::Box {
    let outer = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .hexpand(true)
        .vexpand(true)
        .build();

    // Toolbar
    let toolbar = build_toolbar();
    outer.append(&toolbar);

    // Notebook (tabs)
    let notebook = gtk::Notebook::new();
    notebook.set_scrollable(true);
    notebook.set_show_border(false);
    notebook.set_hexpand(true);
    notebook.set_vexpand(true);
    notebook.popup_enable();
    outer.append(&notebook);

    // Add first terminal tab
    let term = add_terminal_tab(&notebook, working_directory, &callbacks);

    // Toolbar button actions
    let nb = notebook.clone();
    let cb = callbacks.clone();
    connect_toolbar_buttons(&toolbar, &outer, &nb, cb);

    outer
}

// ---------------------------------------------------------------------------
// Toolbar
// ---------------------------------------------------------------------------

fn build_toolbar() -> gtk::Box {
    let toolbar = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .spacing(2)
        .build();
    toolbar.add_css_class("cmux-pane-toolbar");

    let term_btn = gtk::Button::builder()
        .label("Terminal")
        .tooltip_text("New terminal tab")
        .build();
    term_btn.set_widget_name("btn-new-terminal");

    let browser_btn = gtk::Button::builder()
        .label("Browser")
        .tooltip_text("New browser tab")
        .build();
    browser_btn.set_widget_name("btn-new-browser");

    let spacer = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    spacer.set_hexpand(true);

    let split_h_btn = gtk::Button::builder()
        .label("⬌") // horizontal split icon
        .tooltip_text("Split right")
        .build();
    split_h_btn.set_widget_name("btn-split-h");

    let split_v_btn = gtk::Button::builder()
        .label("⬍") // vertical split icon
        .tooltip_text("Split down")
        .build();
    split_v_btn.set_widget_name("btn-split-v");

    let close_btn = gtk::Button::builder()
        .label("✕")
        .tooltip_text("Close pane")
        .build();
    close_btn.set_widget_name("btn-close-pane");

    toolbar.append(&term_btn);
    toolbar.append(&browser_btn);
    toolbar.append(&spacer);
    toolbar.append(&split_h_btn);
    toolbar.append(&split_v_btn);
    toolbar.append(&close_btn);

    toolbar
}

fn connect_toolbar_buttons(
    toolbar: &gtk::Box,
    pane_widget: &gtk::Box,
    notebook: &gtk::Notebook,
    callbacks: Rc<PaneCallbacks>,
) {
    // Walk toolbar children to find buttons by name
    let mut child = toolbar.first_child();
    while let Some(widget) = child {
        let next = widget.next_sibling();
        if let Some(btn) = widget.downcast_ref::<gtk::Button>() {
            let name = btn.widget_name().to_string();
            match name.as_str() {
                "btn-new-terminal" => {
                    let nb = notebook.clone();
                    let cb = callbacks.clone();
                    btn.connect_clicked(move |_| {
                        let term = add_terminal_tab(&nb, None, &cb);
                        // Switch to the new tab
                        if let Some(page) = term.parent() {
                            let page_num = nb.page_num(&page).or_else(|| nb.page_num(&term.clone().upcast::<gtk::Widget>()));
                            if let Some(n) = page_num {
                                nb.set_current_page(Some(n));
                            }
                        }
                    });
                }
                "btn-new-browser" => {
                    let nb = notebook.clone();
                    btn.connect_clicked(move |_| {
                        add_browser_tab(&nb);
                    });
                }
                "btn-split-h" => {
                    let pw = pane_widget.clone();
                    let cb = callbacks.clone();
                    btn.connect_clicked(move |_| {
                        (cb.on_split)(&pw.clone().upcast(), gtk::Orientation::Horizontal);
                    });
                }
                "btn-split-v" => {
                    let pw = pane_widget.clone();
                    let cb = callbacks.clone();
                    btn.connect_clicked(move |_| {
                        (cb.on_split)(&pw.clone().upcast(), gtk::Orientation::Vertical);
                    });
                }
                "btn-close-pane" => {
                    let pw = pane_widget.clone();
                    let cb = callbacks.clone();
                    btn.connect_clicked(move |_| {
                        (cb.on_close_pane)(&pw.clone().upcast());
                    });
                }
                _ => {}
            }
        }
        child = next;
    }
}

// ---------------------------------------------------------------------------
// Tab management
// ---------------------------------------------------------------------------

/// Add a new terminal tab to the notebook. Returns the terminal widget.
pub fn add_terminal_tab(
    notebook: &gtk::Notebook,
    working_directory: Option<&str>,
    callbacks: &Rc<PaneCallbacks>,
) -> vte4::Terminal {
    let term = terminal::create_terminal(working_directory);

    // Tab label with close button
    let tab_label = build_tab_label("Terminal", notebook, &term.clone().upcast());

    // Bell notification
    {
        let cb = callbacks.clone();
        term.connect_bell(move |_: &vte4::Terminal| {
            (cb.on_bell)();
        });
    }

    // Update tab label when terminal title changes
    {
        let tab_label_text = tab_label.1.clone();
        term.connect_window_title_notify(move |t: &vte4::Terminal| {
            if let Some(title) = t.window_title() {
                let title_str: String = title.into();
                if !title_str.is_empty() {
                    // Truncate for tab display
                    let display = if title_str.len() > 25 {
                        format!("{}…", &title_str[..24])
                    } else {
                        title_str
                    };
                    tab_label_text.set_label(&display);
                }
            }
        });
    }

    // On child exit, remove the tab
    {
        let nb = notebook.clone();
        let callbacks = callbacks.clone();
        term.connect_child_exited(move |t: &vte4::Terminal, _status: i32| {
            let t_widget: gtk::Widget = t.clone().upcast();
            let nb2 = nb.clone();
            let callbacks = callbacks.clone();
            glib::idle_add_local_once(move || {
                if let Some(page_num) = nb2.page_num(&t_widget) {
                    nb2.remove_page(Some(page_num));
                }
                // If notebook is empty, signal pane should close
                if nb2.n_pages() == 0 {
                    if let Some(parent) = nb2.parent() {
                        (callbacks.on_empty)(&parent);
                    }
                }
            });
        });
    }

    let page_num = notebook.append_page(&term, Some(&tab_label.0));
    notebook.set_tab_reorderable(&term, true);
    notebook.set_current_page(Some(page_num));

    // Focus the terminal
    term.grab_focus();

    term
}

/// Add a browser tab. Returns the WebView widget (or a placeholder if webkit6 unavailable).
pub fn add_browser_tab(notebook: &gtk::Notebook) -> gtk::Widget {
    // Try to create a real WebKit browser, fall back to placeholder
    let (widget, tab_title) = create_browser_widget();

    let tab_label = build_tab_label(&tab_title, notebook, &widget);

    let page_num = notebook.append_page(&widget, Some(&tab_label.0));
    notebook.set_tab_reorderable(&widget, true);
    notebook.set_current_page(Some(page_num));

    widget
}

#[cfg(feature = "webkit")]
fn create_browser_widget() -> (gtk::Widget, String) {
    use webkit6::prelude::*;
    use webkit6::WebView;

    let webview = WebView::new();
    webview.set_hexpand(true);
    webview.set_vexpand(true);
    webview.load_uri("https://google.com");

    // Wrap in a box with an address bar
    let url_entry = gtk::Entry::builder()
        .placeholder_text("Enter URL...")
        .hexpand(true)
        .build();

    let back_btn = gtk::Button::builder().label("◀").build();
    let fwd_btn = gtk::Button::builder().label("▶").build();
    let reload_btn = gtk::Button::builder().label("⟳").build();

    let nav_bar = gtk::Box::new(gtk::Orientation::Horizontal, 4);
    nav_bar.add_css_class("cmux-pane-toolbar");
    nav_bar.append(&back_btn);
    nav_bar.append(&fwd_btn);
    nav_bar.append(&reload_btn);
    nav_bar.append(&url_entry);

    // Wire navigation
    {
        let wv = webview.clone();
        back_btn.connect_clicked(move |_| { wv.go_back(); });
    }
    {
        let wv = webview.clone();
        fwd_btn.connect_clicked(move |_| { wv.go_forward(); });
    }
    {
        let wv = webview.clone();
        reload_btn.connect_clicked(move |_| { wv.reload(); });
    }
    {
        let wv = webview.clone();
        url_entry.connect_activate(move |entry| {
            let mut url = entry.text().to_string();
            if !url.starts_with("http://") && !url.starts_with("https://") {
                if url.contains('.') {
                    url = format!("https://{url}");
                } else {
                    url = format!("https://www.google.com/search?q={}", url.replace(' ', "+"));
                }
            }
            wv.load_uri(&url);
        });
    }
    // Sync URL bar
    {
        let entry = url_entry.clone();
        webview.connect_uri_notify(move |wv: &WebView| {
            if let Some(uri) = wv.uri() {
                let uri_str: String = uri.into();
                entry.set_text(&uri_str);
            }
        });
    }

    let vbox = gtk::Box::new(gtk::Orientation::Vertical, 0);
    vbox.append(&nav_bar);
    vbox.append(&webview);
    vbox.set_hexpand(true);
    vbox.set_vexpand(true);

    (vbox.upcast(), "Browser".to_string())
}

#[cfg(not(feature = "webkit"))]
fn create_browser_widget() -> (gtk::Widget, String) {
    let placeholder = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .halign(gtk::Align::Center)
        .valign(gtk::Align::Center)
        .spacing(12)
        .build();

    let icon = gtk::Label::builder()
        .label("🌐")
        .build();
    icon.set_css_classes(&["title-1"]);

    let msg = gtk::Label::builder()
        .label("Browser requires webkit6")
        .build();
    msg.set_css_classes(&["dim-label"]);

    let hint = gtk::Label::builder()
        .label("Install: sudo apt install libwebkitgtk-6.0-dev\nThen rebuild with: cargo build --features webkit")
        .justify(gtk::Justification::Center)
        .build();
    hint.set_css_classes(&["dim-label"]);

    placeholder.append(&icon);
    placeholder.append(&msg);
    placeholder.append(&hint);
    placeholder.set_hexpand(true);
    placeholder.set_vexpand(true);

    (placeholder.upcast(), "Browser".to_string())
}

// ---------------------------------------------------------------------------
// Tab label with close button
// ---------------------------------------------------------------------------

/// Returns (container_box, title_label) — the label so callers can update the title.
fn build_tab_label(
    title: &str,
    notebook: &gtk::Notebook,
    tab_content: &gtk::Widget,
) -> (gtk::Box, gtk::Label) {
    let label = gtk::Label::builder()
        .label(title)
        .ellipsize(gtk::pango::EllipsizeMode::End)
        .max_width_chars(20)
        .build();

    let close_btn = gtk::Button::builder()
        .icon_name("window-close-symbolic")
        .has_frame(false)
        .build();
    close_btn.set_css_classes(&["flat", "circular"]);

    let hbox = gtk::Box::new(gtk::Orientation::Horizontal, 4);
    hbox.append(&label);
    hbox.append(&close_btn);

    // Close button removes this tab
    {
        let nb = notebook.clone();
        let content = tab_content.clone();
        close_btn.connect_clicked(move |_| {
            if let Some(page_num) = nb.page_num(&content) {
                nb.remove_page(Some(page_num));
            }
        });
    }

    (hbox, label)
}
