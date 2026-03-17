use gtk4 as gtk;
use gtk::glib;
use gtk::prelude::*;

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::ffi::CString;
use std::os::raw::{c_char, c_int, c_void};
use std::ptr;
use std::rc::Rc;
use std::sync::OnceLock;

use cmux_ghostty_sys::*;

// ---------------------------------------------------------------------------
// Global Ghostty app singleton
// ---------------------------------------------------------------------------

struct GhosttyState {
    app: ghostty_app_t,
}

// Safety: ghostty_app_t is thread-safe for the operations we perform
unsafe impl Send for GhosttyState {}
unsafe impl Sync for GhosttyState {}

static GHOSTTY: OnceLock<GhosttyState> = OnceLock::new();

/// Per-surface state, stored in a global registry keyed by surface pointer.
struct SurfaceEntry {
    gl_area: gtk::GLArea,
    on_title_changed: Option<Box<dyn Fn(&str)>>,
    on_bell: Option<Box<dyn Fn()>>,
    on_close: Option<Box<dyn Fn()>>,
}

thread_local! {
    static SURFACE_MAP: RefCell<HashMap<usize, SurfaceEntry>> = RefCell::new(HashMap::new());
}

/// Initialize the global Ghostty app. Must be called once before creating surfaces.
pub fn init_ghostty() {
    GHOSTTY.get_or_init(|| {
        unsafe {
            ghostty_init(0, ptr::null_mut());
        }

        let config = unsafe {
            let c = ghostty_config_new();
            ghostty_config_load_default_files(c);
            ghostty_config_load_recursive_files(c);
            ghostty_config_finalize(c);
            c
        };

        let runtime_config = ghostty_runtime_config_s {
            userdata: ptr::null_mut(),
            supports_selection_clipboard: true,
            wakeup_cb: ghostty_wakeup_cb,
            action_cb: ghostty_action_cb,
            read_clipboard_cb: ghostty_read_clipboard_cb,
            confirm_read_clipboard_cb: ghostty_confirm_read_clipboard_cb,
            write_clipboard_cb: ghostty_write_clipboard_cb,
            close_surface_cb: ghostty_close_surface_cb,
        };

        let app = unsafe { ghostty_app_new(&runtime_config, config) };

        // Ghostty's GTK apprt calls core_app.tick() on every GLib main
        // loop iteration to drain the app mailbox (which includes
        // redraw_surface messages from the renderer thread). The renderer
        // thread pushes these messages but doesn't wake the app.
        // We replicate this with a high-frequency timer (~8ms ≈ 120Hz).
        glib::timeout_add_local(std::time::Duration::from_millis(8), move || {
            unsafe { ghostty_app_tick(app) };
            glib::ControlFlow::Continue
        });

        GhosttyState { app }
    });
}

fn ghostty_app() -> ghostty_app_t {
    GHOSTTY.get().expect("ghostty not initialized").app
}

// ---------------------------------------------------------------------------
// Runtime callbacks (C ABI)
// ---------------------------------------------------------------------------

unsafe extern "C" fn ghostty_wakeup_cb(_userdata: *mut c_void) {
    glib::idle_add_once(|| {
        let app = ghostty_app();
        unsafe { ghostty_app_tick(app) };
    });
}

unsafe extern "C" fn ghostty_action_cb(
    _app: ghostty_app_t,
    target: ghostty_target_s,
    action: ghostty_action_s,
) -> bool {
    let tag = action.tag;

    match tag {
        GHOSTTY_ACTION_RENDER => {
            if target.tag == GHOSTTY_TARGET_SURFACE {
                let surface_key = unsafe { target.target.surface } as usize;
                SURFACE_MAP.with(|map| {
                    if let Some(entry) = map.borrow().get(&surface_key) {
                        entry.gl_area.queue_render();
                    }
                });
            }
            true
        }
        GHOSTTY_ACTION_SET_TITLE => {
            if target.tag == GHOSTTY_TARGET_SURFACE {
                let surface_key = unsafe { target.target.surface } as usize;
                let title_ptr = unsafe { action.action.set_title.title };
                if !title_ptr.is_null() {
                    let title = unsafe { std::ffi::CStr::from_ptr(title_ptr) }
                        .to_str()
                        .unwrap_or("")
                        .to_string();
                    SURFACE_MAP.with(|map| {
                        if let Some(entry) = map.borrow().get(&surface_key) {
                            if let Some(cb) = &entry.on_title_changed {
                                cb(&title);
                            }
                        }
                    });
                }
            }
            true
        }
        GHOSTTY_ACTION_RING_BELL => {
            if target.tag == GHOSTTY_TARGET_SURFACE {
                let surface_key = unsafe { target.target.surface } as usize;
                SURFACE_MAP.with(|map| {
                    if let Some(entry) = map.borrow().get(&surface_key) {
                        if let Some(cb) = &entry.on_bell {
                            cb();
                        }
                    }
                });
            }
            true
        }
        GHOSTTY_ACTION_SHOW_CHILD_EXITED => {
            if target.tag == GHOSTTY_TARGET_SURFACE {
                let surface_key = unsafe { target.target.surface } as usize;
                glib::idle_add_local_once(move || {
                    SURFACE_MAP.with(|map| {
                        if let Some(entry) = map.borrow().get(&surface_key) {
                            if let Some(cb) = &entry.on_close {
                                cb();
                            }
                        }
                    });
                });
            }
            true
        }
        _ => false,
    }
}

unsafe extern "C" fn ghostty_read_clipboard_cb(
    userdata: *mut c_void,
    clipboard_type: c_int,
    state: *mut c_void,
) {
    let surface_key = userdata as usize;
    let display = match gtk::gdk::Display::default() {
        Some(d) => d,
        None => return,
    };
    let clipboard = if clipboard_type == GHOSTTY_CLIPBOARD_SELECTION {
        display.primary_clipboard()
    } else {
        display.clipboard()
    };

    clipboard.read_text_async(gtk::gio::Cancellable::NONE, move |result| {
        let text = result.ok().flatten().map(|s| s.to_string());
        if let Some(text) = text {
            if let Ok(cstr) = CString::new(text) {
                unsafe {
                    ghostty_surface_complete_clipboard_request(
                        surface_key as ghostty_surface_t,
                        cstr.as_ptr(),
                        state,
                        true,
                    );
                }
            }
        }
    });
}

unsafe extern "C" fn ghostty_confirm_read_clipboard_cb(
    userdata: *mut c_void,
    text: *const c_char,
    state: *mut c_void,
    _request_type: c_int,
) {
    let surface_key = userdata as usize;
    unsafe {
        ghostty_surface_complete_clipboard_request(
            surface_key as ghostty_surface_t,
            text,
            state,
            true,
        );
    }
}

unsafe extern "C" fn ghostty_write_clipboard_cb(
    _userdata: *mut c_void,
    clipboard_type: c_int,
    contents: *const ghostty_clipboard_content_s,
    count: usize,
    _confirm: bool,
) {
    if count == 0 || contents.is_null() {
        return;
    }

    let content = unsafe { &*contents };
    if content.data.is_null() {
        return;
    }
    let text = unsafe { std::ffi::CStr::from_ptr(content.data) }
        .to_str()
        .unwrap_or("")
        .to_string();

    let display = match gtk::gdk::Display::default() {
        Some(d) => d,
        None => return,
    };
    let clipboard = if clipboard_type == GHOSTTY_CLIPBOARD_SELECTION {
        display.primary_clipboard()
    } else {
        display.clipboard()
    };
    clipboard.set_text(&text);
}

unsafe extern "C" fn ghostty_close_surface_cb(userdata: *mut c_void, _process_alive: bool) {
    let surface_key = userdata as usize;
    glib::idle_add_local_once(move || {
        SURFACE_MAP.with(|map| {
            if let Some(entry) = map.borrow().get(&surface_key) {
                if let Some(cb) = &entry.on_close {
                    cb();
                }
            }
        });
    });
}

// ---------------------------------------------------------------------------
// Surface creation
// ---------------------------------------------------------------------------

pub struct TerminalCallbacks {
    pub on_title_changed: Box<dyn Fn(&str)>,
    pub on_bell: Box<dyn Fn()>,
    pub on_close: Box<dyn Fn()>,
}

/// Create a new Ghostty-powered terminal widget (GLArea).
pub fn create_terminal(
    working_directory: Option<&str>,
    callbacks: TerminalCallbacks,
) -> gtk::GLArea {
    let gl_area = gtk::GLArea::new();
    gl_area.set_hexpand(true);
    gl_area.set_vexpand(true);
    // auto_render=true ensures GTK continuously redraws the GLArea,
    // which forces its internal FBO to match the current allocation.
    // With auto_render=false, the FBO may stay at the initial size.
    gl_area.set_auto_render(true);
    gl_area.set_focusable(true);
    gl_area.set_can_focus(true);

    let wd = working_directory.map(|s| s.to_string());
    let callbacks = Rc::new(callbacks);
    let surface_cell: Rc<RefCell<Option<ghostty_surface_t>>> = Rc::new(RefCell::new(None));
    let had_focus = Rc::new(Cell::new(false));

    // On realize: create the Ghostty surface
    {
        let gl = gl_area.clone();
        let surface_cell = surface_cell.clone();
        let callbacks = callbacks.clone();
        let had_focus = had_focus.clone();
        gl_area.connect_realize(move |gl_area| {
            gl_area.make_current();
            if let Some(err) = gl_area.error() {
                eprintln!("cmux: GLArea error after make_current: {err}");
                return;
            }
            let app = ghostty_app();
            let mut config = unsafe { ghostty_surface_config_new() };
            config.platform_tag = GHOSTTY_PLATFORM_LINUX;
            config.platform = ghostty_platform_u {
                linux: ghostty_platform_linux_s {
                    reserved: ptr::null_mut(),
                },
            };

            let scale = gl_area.scale_factor() as f64;
            config.scale_factor = scale;
            config.context = GHOSTTY_SURFACE_CONTEXT_WINDOW;

            let c_wd = wd.as_ref().and_then(|s| CString::new(s.as_str()).ok());
            if let Some(ref cwd) = c_wd {
                config.working_directory = cwd.as_ptr();
            }

            let surface = unsafe { ghostty_surface_new(app, &config) };
            if surface.is_null() {
                eprintln!("cmux: failed to create ghostty surface");
                return;
            }

            // Set initial size — GLArea gives unscaled CSS pixels,
            // Ghostty handles scaling internally via content_scale.
            let alloc = gl_area.allocation();
            let w = alloc.width() as u32;
            let h = alloc.height() as u32;
            if w > 0 && h > 0 {
                unsafe {
                    ghostty_surface_set_content_scale(surface, scale, scale);
                    ghostty_surface_set_size(surface, w, h);
                }
            }

            let surface_key = surface as usize;
            SURFACE_MAP.with(|map| {
                map.borrow_mut().insert(
                    surface_key,
                    SurfaceEntry {
                        gl_area: gl.clone(),
                        on_title_changed: Some(Box::new({
                            let cb = callbacks.clone();
                            move |title| (cb.on_title_changed)(title)
                        })),
                        on_bell: Some(Box::new({
                            let cb = callbacks.clone();
                            move || (cb.on_bell)()
                        })),
                        on_close: Some(Box::new({
                            let cb = callbacks.clone();
                            move || (cb.on_close)()
                        })),
                    },
                );
            });

            *surface_cell.borrow_mut() = Some(surface);

            unsafe {
                ghostty_surface_set_color_scheme(surface, GHOSTTY_COLOR_SCHEME_DARK);
                ghostty_surface_set_focus(surface, true);
            }

            // Grab GTK focus so key events reach this widget
            had_focus.set(true);
            gl_area.grab_focus();
        });
    }

    // On render: draw the surface.
    {
        let surface_cell = surface_cell.clone();
        gl_area.connect_render(move |_gl_area, _context| {
            if let Some(surface) = *surface_cell.borrow() {
                unsafe { ghostty_surface_draw(surface) };
            }
            glib::Propagation::Stop
        });
    }

    // On resize: update Ghostty's terminal grid size and queue a redraw.
    // The actual GL viewport is set by GTK when the render signal fires,
    // so we must NOT call ghostty_surface_draw here — the viewport would
    // still be the old size. Instead we queue_render() and let the render
    // callback draw with the correct viewport.
    {
        let surface_cell = surface_cell.clone();
        let gl_for_resize = gl_area.clone();
        let had_focus = had_focus.clone();
        gl_area.connect_resize(move |gl_area, width, height| {
            if let Some(surface) = *surface_cell.borrow() {
                let w = width as u32;
                let h = height as u32;
                if w > 0 && h > 0 {
                    let scale = gl_area.scale_factor() as f64;
                    unsafe {
                        ghostty_surface_set_content_scale(surface, scale, scale);
                        ghostty_surface_set_size(surface, w, h);
                    }
                    gl_area.queue_render();
                }
            }

            if had_focus.get() {
                let gl_for_focus = gl_for_resize.clone();
                glib::idle_add_local_once(move || {
                    gl_for_focus.grab_focus();
                });
            }
        });
    }

    // Keyboard input
    //
    // Ghostty expects two things for text input:
    // 1. ghostty_surface_key() with the hardware keycode (for keybindings)
    // 2. ghostty_surface_text() with the UTF-8 text (for actual character input)
    //
    // We use an IMMulticontext for proper text composition and send the
    // composed text via ghostty_surface_text(). The key event is sent for
    // keybinding processing.
    {
        let im_context = gtk::IMMulticontext::new();

        // When the IM produces committed text, send it to Ghostty
        {
            let sc = surface_cell.clone();
            im_context.connect_commit(move |_im, text| {
                if let Some(surface) = *sc.borrow() {
                    let bytes = text.as_bytes();
                    unsafe {
                        ghostty_surface_text(
                            surface,
                            bytes.as_ptr() as *const c_char,
                            bytes.len(),
                        );
                    }
                }
            });
        }

        let sc_press = surface_cell.clone();
        let sc_release = surface_cell.clone();
        let im_for_press = im_context.clone();
        let key_controller = gtk::EventControllerKey::new();
        key_controller.set_im_context(Some(&im_context));
        key_controller.connect_key_pressed(move |_ctrl, keyval, keycode, modifier| {
            if let Some(surface) = *sc_press.borrow() {
                // Build the text string for this keypress
                let text_char = keyval.to_unicode();
                let mut text_buf = [0u8; 4];
                let text_str = text_char.map(|c| c.encode_utf8(&mut text_buf) as &str);
                let c_text = text_str.and_then(|s| CString::new(s).ok());

                let mut event = translate_key_event(
                    GHOSTTY_ACTION_PRESS,
                    keyval,
                    keycode,
                    modifier,
                );
                if let Some(ref ct) = c_text {
                    event.text = ct.as_ptr();
                }

                let consumed = unsafe { ghostty_surface_key(surface, event) };
                if consumed {
                    return glib::Propagation::Stop;
                }
            }
            glib::Propagation::Proceed
        });

        key_controller.connect_key_released(move |_ctrl, keyval, keycode, modifier| {
            if let Some(surface) = *sc_release.borrow() {
                let event = translate_key_event(
                    GHOSTTY_ACTION_RELEASE,
                    keyval,
                    keycode,
                    modifier,
                );
                unsafe { ghostty_surface_key(surface, event) };
            }
        });

        gl_area.add_controller(key_controller);
        // Store IM context to keep it alive
        unsafe { gl_area.set_data("cmux-im-context", im_context) };
    }

    // Mouse buttons (also handles click-to-focus)
    {
        let surface_cell = surface_cell.clone();
        let click = gtk::GestureClick::new();
        click.set_button(0); // all buttons
        let sc = surface_cell.clone();
        let gl_for_focus = gl_area.clone();
        let had_focus = had_focus.clone();
        click.connect_pressed(move |gesture, _n, x, y| {
            // Grab keyboard focus on any click
            had_focus.set(true);
            gl_for_focus.grab_focus();
            if let Some(surface) = *sc.borrow() {
                let button = match gesture.current_button() {
                    1 => GHOSTTY_MOUSE_LEFT,
                    2 => GHOSTTY_MOUSE_MIDDLE,
                    3 => GHOSTTY_MOUSE_RIGHT,
                    _ => GHOSTTY_MOUSE_UNKNOWN,
                };
                let mods = translate_mouse_mods(gesture.current_event_state());
                unsafe {
                    ghostty_surface_mouse_pos(surface, x, y, mods);
                    ghostty_surface_mouse_button(surface, GHOSTTY_MOUSE_PRESS, button, mods);
                }
            }
        });
        let sc2 = surface_cell.clone();
        click.connect_released(move |gesture, _n, x, y| {
            if let Some(surface) = *sc2.borrow() {
                let button = match gesture.current_button() {
                    1 => GHOSTTY_MOUSE_LEFT,
                    2 => GHOSTTY_MOUSE_MIDDLE,
                    3 => GHOSTTY_MOUSE_RIGHT,
                    _ => GHOSTTY_MOUSE_UNKNOWN,
                };
                let mods = translate_mouse_mods(gesture.current_event_state());
                unsafe {
                    ghostty_surface_mouse_pos(surface, x, y, mods);
                    ghostty_surface_mouse_button(surface, GHOSTTY_MOUSE_RELEASE, button, mods);
                }
            }
        });
        gl_area.add_controller(click);
    }

    // Mouse motion
    {
        let surface_cell = surface_cell.clone();
        let motion = gtk::EventControllerMotion::new();
        motion.connect_motion(move |ctrl, x, y| {
            if let Some(surface) = *surface_cell.borrow() {
                let mods = translate_mouse_mods(ctrl.current_event_state());
                unsafe { ghostty_surface_mouse_pos(surface, x, y, mods) };
            }
        });
        gl_area.add_controller(motion);
    }

    // Mouse scroll
    {
        let surface_cell = surface_cell.clone();
        let scroll = gtk::EventControllerScroll::new(
            gtk::EventControllerScrollFlags::BOTH_AXES
                | gtk::EventControllerScrollFlags::DISCRETE,
        );
        scroll.connect_scroll(move |ctrl, dx, dy| {
            if let Some(surface) = *surface_cell.borrow() {
                let mods = translate_mouse_mods(ctrl.current_event_state());
                unsafe { ghostty_surface_mouse_scroll(surface, dx, dy, mods) };
            }
            glib::Propagation::Stop
        });
        gl_area.add_controller(scroll);
    }

    // Focus
    {
        let surface_cell = surface_cell.clone();
        let had_focus_enter = had_focus.clone();
        let had_focus_leave = had_focus.clone();
        let focus_ctrl = gtk::EventControllerFocus::new();
        let sc = surface_cell.clone();
        focus_ctrl.connect_enter(move |_| {
            had_focus_enter.set(true);
            if let Some(surface) = *sc.borrow() {
                unsafe { ghostty_surface_set_focus(surface, true) };
            }
        });
        focus_ctrl.connect_leave(move |_| {
            had_focus_leave.set(false);
            if let Some(surface) = *surface_cell.borrow() {
                unsafe { ghostty_surface_set_focus(surface, false) };
            }
        });
        gl_area.add_controller(focus_ctrl);
    }

    // Clean up on unrealize
    {
        let surface_cell = surface_cell.clone();
        gl_area.connect_unrealize(move |_| {
            if let Some(surface) = surface_cell.borrow_mut().take() {
                let surface_key = surface as usize;
                SURFACE_MAP.with(|map| {
                    map.borrow_mut().remove(&surface_key);
                });
                unsafe { ghostty_surface_free(surface) };
            }
        });
    }

    gl_area
}

// ---------------------------------------------------------------------------
// Key translation
// ---------------------------------------------------------------------------

fn translate_key_event(
    action: c_int,
    keyval: gtk::gdk::Key,
    keycode: u32,
    modifier: gtk::gdk::ModifierType,
) -> ghostty_input_key_s {
    let mut mods: c_int = GHOSTTY_MODS_NONE;
    if modifier.contains(gtk::gdk::ModifierType::SHIFT_MASK) {
        mods |= GHOSTTY_MODS_SHIFT;
    }
    if modifier.contains(gtk::gdk::ModifierType::CONTROL_MASK) {
        mods |= GHOSTTY_MODS_CTRL;
    }
    if modifier.contains(gtk::gdk::ModifierType::ALT_MASK) {
        mods |= GHOSTTY_MODS_ALT;
    }
    if modifier.contains(gtk::gdk::ModifierType::SUPER_MASK) {
        mods |= GHOSTTY_MODS_SUPER;
    }

    let codepoint = keyval.to_unicode().map(|c| c as u32).unwrap_or(0);

    ghostty_input_key_s {
        action,
        mods,
        consumed_mods: GHOSTTY_MODS_NONE,
        keycode,
        text: ptr::null(),
        unshifted_codepoint: codepoint,
        composing: false,
    }
}

fn translate_mouse_mods(state: gtk::gdk::ModifierType) -> c_int {
    let mut mods: c_int = GHOSTTY_MODS_NONE;
    if state.contains(gtk::gdk::ModifierType::SHIFT_MASK) {
        mods |= GHOSTTY_MODS_SHIFT;
    }
    if state.contains(gtk::gdk::ModifierType::CONTROL_MASK) {
        mods |= GHOSTTY_MODS_CTRL;
    }
    if state.contains(gtk::gdk::ModifierType::ALT_MASK) {
        mods |= GHOSTTY_MODS_ALT;
    }
    if state.contains(gtk::gdk::ModifierType::SUPER_MASK) {
        mods |= GHOSTTY_MODS_SUPER;
    }
    mods
}
