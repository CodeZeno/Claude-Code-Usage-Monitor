use super::*;

pub(super) fn position_at_taskbar() {
    let should_skip = {
        let state = lock_state();
        state.as_ref().is_some_and(|s| s.dragging || s.auto_ejected)
    };
    if should_skip {
        return;
    }
    refresh_dpi();
    let custom_position = {
        let state = lock_state();
        state.as_ref().and_then(|s| {
            if s.custom_theme_enabled {
                effective_theme_from_state(s).map(|mut theme| {
                    let runtime = theme_runtime_for_surface(&theme, 0, theme_runtime_from_state(s));
                    let (width, height) =
                        theme_engine::resolve_surface_size(&theme, 0, s.data.as_ref(), runtime);
                    theme.canvas.width = width;
                    theme.canvas.height = height;
                    let scale = theme_surface_scale(&theme, 0);
                    (s.hwnd.to_hwnd(), theme, scale)
                })
            } else {
                None
            }
        })
    };
    if let Some((hwnd, theme, scale)) = custom_position {
        position_custom_theme(hwnd, &theme, scale);
        return;
    }
    // Drop the app-state lock before any Win32 call that may synchronously
    // re-enter our window procedure.
    let (hwnd, embedded, tray_offset, taskbar_hwnd) = {
        let state = lock_state();
        let s = match state.as_ref() {
            Some(s) => s,
            None => return,
        };

        // Don't fight the user's drag
        if s.dragging {
            return;
        }

        let taskbar_hwnd = match s.taskbar_hwnd {
            Some(h) => h.to_hwnd(),
            None => {
                diagnose::log("position_at_taskbar skipped: no taskbar handle");
                return;
            }
        };

        (s.hwnd.to_hwnd(), s.embedded, s.tray_offset, taskbar_hwnd)
    };

    let taskbar_rect = match native_interop::get_taskbar_rect(taskbar_hwnd) {
        Some(r) => r,
        None => {
            diagnose::log("position_at_taskbar skipped: unable to query taskbar rect");
            return;
        }
    };

    let taskbar_height = taskbar_rect.bottom - taskbar_rect.top;
    let mut tray_left = taskbar_rect.right;
    let anchor_top = taskbar_rect.top;
    let anchor_height = taskbar_height;

    if let Some(tray_hwnd) = native_interop::find_child_window(taskbar_hwnd, "TrayNotifyWnd") {
        if let Some(tray_rect) = native_interop::get_window_rect_safe(tray_hwnd) {
            tray_left = tray_rect.left;
        }
    }

    let widget_width = total_widget_width();
    let max_offset = (tray_left - taskbar_rect.left - widget_width).max(0);
    let tray_offset = tray_offset.clamp(0, max_offset);
    let offset_changed = {
        let mut state = lock_state();
        if let Some(s) = state.as_mut() {
            if s.tray_offset != tray_offset {
                s.tray_offset = tray_offset;
                true
            } else {
                false
            }
        } else {
            false
        }
    };
    if offset_changed {
        save_state_settings();
    }

    let widget_height = total_widget_height();
    let y = compute_anchor_y(anchor_top, anchor_height, widget_height);
    if embedded {
        // Child window: coordinates relative to parent (taskbar)
        let x = tray_left - taskbar_rect.left - widget_width - tray_offset;
        native_interop::move_window(hwnd, x, y - taskbar_rect.top, widget_width, widget_height);
        diagnose::log(format!(
            "positioned embedded widget at x={x} y={} w={widget_width} h={widget_height}",
            y - taskbar_rect.top
        ));
    } else {
        // Topmost popup: screen coordinates
        let x = tray_left - widget_width - tray_offset;
        native_interop::move_window(hwnd, x, y, widget_width, widget_height);
        diagnose::log(format!(
            "positioned fallback widget at x={x} y={y} w={widget_width} h={widget_height}"
        ));
    }
}

pub(super) fn ensure_layered_window(hwnd: HWND) {
    unsafe {
        let ex_style = GetWindowLongW(hwnd, GWL_EXSTYLE);
        if ex_style & WS_EX_LAYERED.0 as i32 == 0 {
            let _ = SetWindowLongW(hwnd, GWL_EXSTYLE, ex_style | WS_EX_LAYERED.0 as i32);
        }
    }
}

pub(super) fn render_desktop_custom_window(hwnd: HWND, rendered: &theme_engine::RenderedTheme) {
    if let Err(error) = crate::desktop_compositor::present(hwnd, rendered) {
        diagnose::log(format!(
            "desktop theme render failed hwnd={:?} size={}x{} error={error}",
            hwnd, rendered.width, rendered.height
        ));
    }
}

pub(super) fn render_custom_window(
    hwnd: HWND,
    rendered: &theme_engine::RenderedTheme,
    desktop_nested: bool,
) {
    if desktop_nested {
        render_desktop_custom_window(hwnd, rendered);
        return;
    }

    let width = rendered.width as i32;
    let height = rendered.height as i32;
    unsafe {
        // Keep the DWM surface alive across frames. Desktop rendering uses a
        // separate DirectComposition window, so no layered-style reset is
        // needed here; reparenting resets it once in embed_as_child instead.
        ensure_layered_window(hwnd);
        // UpdateLayeredWindow expects a screen-compatible destination DC. A
        // window DC happened to work for taskbar-hosted children, but desktop
        // WorkerW/DefView composition can discard the resulting surface.
        let screen_dc = GetDC(None);
        let memory_dc = CreateCompatibleDC(Some(screen_dc));
        let info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: width,
                biHeight: -height,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut bits = std::ptr::null_mut();
        let bitmap = CreateDIBSection(Some(memory_dc), &info, DIB_RGB_COLORS, &mut bits, None, 0)
            .unwrap_or_default();
        if bitmap.is_invalid() || bits.is_null() {
            let _ = DeleteDC(memory_dc);
            ReleaseDC(None, screen_dc);
            return;
        }
        let old = SelectObject(memory_dc, bitmap.into());
        let window_pixels = std::slice::from_raw_parts_mut(bits as *mut u32, rendered.pixels.len());
        for (target, source) in window_pixels.iter_mut().zip(&rendered.pixels) {
            // Windows normally lets mouse input pass through zero-alpha pixels in
            // layered windows. A nearly transparent pixel keeps the full surface
            // interactive without changing the theme renderer's pixel output.
            *target = if source >> 24 == 0 {
                0x0100_0000
            } else {
                *source
            };
        }
        let source = POINT { x: 0, y: 0 };
        let size = SIZE {
            cx: width,
            cy: height,
        };
        let blend = BLENDFUNCTION {
            BlendOp: AC_SRC_OVER as u8,
            BlendFlags: 0,
            SourceConstantAlpha: 255,
            AlphaFormat: AC_SRC_ALPHA as u8,
        };
        if let Err(error) = UpdateLayeredWindow(
            hwnd,
            Some(screen_dc),
            None,
            Some(&size),
            Some(memory_dc),
            Some(&source),
            COLORREF(0),
            Some(&blend),
            ULW_ALPHA,
        ) {
            diagnose::log(format!(
                "custom theme render failed hwnd={:?} size={}x{} error={error}",
                hwnd, width, height
            ));
        }
        SelectObject(memory_dc, old);
        let _ = DeleteObject(bitmap.into());
        let _ = DeleteDC(memory_dc);
        ReleaseDC(None, screen_dc);
    }
}

pub(super) fn position_custom_theme(hwnd: HWND, theme: &ThemeDocument, scale: f64) {
    position_custom_theme_internal(hwnd, theme, scale);
}

pub(super) fn position_custom_theme_internal(hwnd: HWND, theme: &ThemeDocument, scale: f64) {
    let is_dragging = {
        let state = lock_state();
        state.as_ref().is_some_and(|s| s.dragging)
    };
    if is_dragging {
        return;
    }
    let taskbars = native_interop::find_taskbars();
    let displays = native_interop::find_monitors();
    let display_index = theme.placement.reference.display;
    let selected_display = displays
        .get(display_index)
        .copied()
        .or_else(|| displays.first().copied());
    let Some(display) = selected_display else {
        return;
    };
    let taskbar = taskbars.iter().find(|taskbar| unsafe {
        MonitorFromWindow(taskbar.hwnd, MONITOR_DEFAULTTOPRIMARY) == display.handle
    });
    let width = scaled_theme_dimension(theme.canvas.width.max(1), scale);
    let height = scaled_theme_dimension(theme.canvas.height.max(1), scale);
    let tray = taskbar
        .and_then(|tb| native_interop::find_child_window(tb.hwnd, "TrayNotifyWnd"))
        .and_then(native_interop::get_window_rect_safe);
    let rect = surface_screen_rect(
        &theme.placement,
        width,
        height,
        scale,
        display.rect,
        taskbar.map(|tb| tb.rect),
        tray,
    );
    let (x, y) = (rect.left, rect.top);
    let nest = theme
        .placement
        .nest
        .resolve(theme.placement.reference.region);
    unsafe {
        match nest {
            SurfaceNest::Taskbar => {
                let Some(taskbar) = taskbar else {
                    let _ = ShowWindow(hwnd, SW_HIDE);
                    return;
                };
                native_interop::embed_as_child(hwnd, taskbar.hwnd);
                ensure_tray_event_hook_for_taskbar(hwnd, taskbar.hwnd);
                let mut point = [POINT { x, y }];
                MapWindowPoints(None, Some(taskbar.hwnd), &mut point);
                let _ = SetWindowPos(
                    hwnd,
                    Some(HWND_TOP),
                    point[0].x,
                    point[0].y,
                    width,
                    height,
                    SWP_NOACTIVATE,
                );
            }
            SurfaceNest::Desktop => {
                if let Some(desktop) = native_interop::find_desktop_host() {
                    if GetParent(hwnd).ok() != Some(desktop.parent) {
                        native_interop::embed_as_child(hwnd, desktop.parent);
                    }
                    let mut point = [POINT { x, y }];
                    MapWindowPoints(None, Some(desktop.parent), &mut point);
                    let _ = SetWindowPos(
                        hwnd,
                        Some(desktop.insert_after),
                        point[0].x,
                        point[0].y,
                        width,
                        height,
                        SWP_NOACTIVATE,
                    );
                } else {
                    native_interop::make_popup(hwnd, false);
                    let _ =
                        SetWindowPos(hwnd, Some(HWND_BOTTOM), x, y, width, height, SWP_NOACTIVATE);
                }
            }
            SurfaceNest::TrayIcon => {
                let _ = ShowWindow(hwnd, SW_HIDE);
            }
            SurfaceNest::Floating | SurfaceNest::Auto => {
                native_interop::make_popup(hwnd, true);
                let _ = SetWindowPos(
                    hwnd,
                    Some(HWND_TOPMOST),
                    x,
                    y,
                    width,
                    height,
                    SWP_NOACTIVATE,
                );
            }
        }
    }
}

pub(super) fn sync_theme_window_visibility() {
    let (theme, data, runtime, windows) = {
        let state = lock_state();
        let Some(state) = state.as_ref() else {
            return;
        };
        if !state.custom_theme_enabled {
            return;
        }
        let Some(theme) = effective_theme_from_state(state) else {
            return;
        };
        (
            theme,
            state.data.clone(),
            theme_runtime_from_state(state),
            std::iter::once(state.hwnd)
                .chain(state.mirror_hwnds.iter().copied())
                .collect::<Vec<_>>(),
        )
    };
    unsafe {
        for (surface_index, surface) in theme.surfaces.iter().enumerate() {
            let nest = surface
                .placement
                .nest
                .resolve(surface.placement.reference.region);
            if nest != SurfaceNest::Floating {
                continue;
            }
            let Some(regular_window) = windows.get(surface_index) else {
                continue;
            };
            let hwnd = regular_window.to_hwnd();
            if !IsWindow(Some(hwnd)).as_bool() {
                continue;
            }
            let surface_runtime = theme_runtime_for_surface(&theme, surface_index, runtime);
            let should_show =
                theme_engine::surface_should_render(
                    &theme,
                    surface_index,
                    data.as_ref(),
                    surface_runtime,
                ) && !foreground_is_fullscreen_on_display(surface.placement.reference.display);
            if should_show == IsWindowVisible(hwnd).as_bool() {
                continue;
            }
            if should_show {
                let _ = SetWindowPos(
                    hwnd,
                    Some(HWND_TOPMOST),
                    0,
                    0,
                    0,
                    0,
                    SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
                );
            }
            let _ = ShowWindow(
                hwnd,
                if should_show {
                    SW_SHOWNOACTIVATE
                } else {
                    SW_HIDE
                },
            );
        }
    }
}

pub(super) fn foreground_is_fullscreen_on_display(display_index: usize) -> bool {
    unsafe {
        let foreground = GetForegroundWindow();
        if foreground.is_invalid()
            || !IsWindowVisible(foreground).as_bool()
            || IsIconic(foreground).as_bool()
        {
            return false;
        }
        let class = native_interop::window_class_name(foreground).unwrap_or_default();
        if matches!(
            class.as_str(),
            "Progman" | "WorkerW" | "Shell_TrayWnd" | "Shell_SecondaryTrayWnd"
        ) {
            return false;
        }
        let is_ours = {
            let state = lock_state();
            state.as_ref().is_some_and(|state| {
                state.hwnd.to_hwnd() == foreground
                    || state
                        .mirror_hwnds
                        .iter()
                        .any(|window| window.to_hwnd() == foreground)
                    || state
                        .desktop_hwnds
                        .iter()
                        .flatten()
                        .any(|window| window.to_hwnd() == foreground)
            })
        };
        if is_ours {
            return false;
        }

        let displays = native_interop::find_monitors();
        let Some(display) = displays
            .get(display_index)
            .copied()
            .or_else(|| displays.first().copied())
        else {
            return false;
        };
        if MonitorFromWindow(foreground, MONITOR_DEFAULTTONULL) != display.handle {
            return false;
        }
        let mut rect = RECT::default();
        if DwmGetWindowAttribute(
            foreground,
            DWMWA_EXTENDED_FRAME_BOUNDS,
            &mut rect as *mut _ as *mut std::ffi::c_void,
            std::mem::size_of::<RECT>() as u32,
        )
        .is_err()
            && GetWindowRect(foreground, &mut rect).is_err()
        {
            return false;
        }
        rect_covers_monitor(rect, display.rect)
    }
}

pub(super) fn rect_covers_monitor(rect: RECT, monitor: RECT) -> bool {
    const EDGE_TOLERANCE: i32 = 2;
    rect.left <= monitor.left + EDGE_TOLERANCE
        && rect.top <= monitor.top + EDGE_TOLERANCE
        && rect.right >= monitor.right - EDGE_TOLERANCE
        && rect.bottom >= monitor.bottom - EDGE_TOLERANCE
}

pub(super) fn aligned_origin(
    reference_start: i32,
    reference_length: i32,
    surface_length: i32,
    reference_factor: f64,
    surface_factor: f64,
    offset: i32,
) -> i32 {
    (reference_start as f64 + reference_length as f64 * reference_factor
        - surface_length as f64 * surface_factor)
        .round() as i32
        + offset
}

pub(super) fn horizontal_anchor_factor(anchor: HorizontalAnchor) -> f64 {
    match anchor {
        HorizontalAnchor::Left => 0.0,
        HorizontalAnchor::Center => 0.5,
        HorizontalAnchor::Right => 1.0,
    }
}

pub(super) fn vertical_anchor_factor(anchor: VerticalAnchor) -> f64 {
    match anchor {
        VerticalAnchor::Top => 0.0,
        VerticalAnchor::Center => 0.5,
        VerticalAnchor::Bottom => 1.0,
    }
}

pub(super) fn compute_anchor_y(anchor_top: i32, anchor_height: i32, widget_height: i32) -> i32 {
    let anchor_bottom = anchor_top + anchor_height;
    (anchor_bottom - widget_height).max(anchor_top)
}

/// Only the primary window owns the watchdog's docking state. Mirrors may
/// share its shell thread, but must never replace its target taskbar.
pub(super) fn record_primary_taskbar(
    state: &mut AppState,
    surface: HWND,
    taskbar: HWND,
    tray: Option<HWND>,
) -> bool {
    if surface != state.hwnd.to_hwnd() {
        return false;
    }
    state.taskbar_hwnd = Some(SendHwnd::from_hwnd(taskbar));
    state.tray_notify_hwnd = tray.map(SendHwnd::from_hwnd);
    state.embedded = true;
    true
}

pub(super) fn ensure_tray_event_hook_for_taskbar(surface: HWND, taskbar: HWND) {
    let tray = native_interop::find_child_window(taskbar, "TrayNotifyWnd");
    let (old_hook, needed) = {
        let mut state = lock_state();
        let Some(state) = state.as_mut() else {
            return;
        };
        let changed = state.taskbar_hwnd.map(SendHwnd::to_hwnd) != Some(taskbar);
        if !record_primary_taskbar(state, surface, taskbar, tray) {
            return;
        }
        let old = if changed {
            state.win_event_hook.take()
        } else {
            None
        };
        (old, state.win_event_hook.is_none())
    };
    if let Some(hook) = old_hook {
        native_interop::unhook_win_event(hook.to_hook());
    }
    if needed {
        // Secondary taskbars need events even when they have no TrayNotifyWnd.
        let thread = native_interop::get_window_thread_id(taskbar);
        let hook = native_interop::set_tray_event_hook(thread, on_tray_location_changed);
        if let Some(state) = lock_state().as_mut() {
            state.win_event_hook = hook.map(SendWinEventHook::from_hook);
        }
    }
}

pub(super) fn rect_changed(previous: Option<RECT>, current: Option<RECT>) -> bool {
    match (previous, current) {
        (Some(p), Some(c)) => {
            p.left != c.left || p.top != c.top || p.right != c.right || p.bottom != c.bottom
        }
        (None, None) => false,
        _ => true,
    }
}

pub(super) fn is_tray_event_source(
    hwnd: HWND,
    tray_hwnd: Option<HWND>,
    taskbar_hwnd: Option<HWND>,
    our_hwnds: &[HWND],
) -> bool {
    if hwnd.is_invalid() {
        return false;
    }
    // Never treat our own surface windows as tray events (prevents feedback loops)
    for &our in our_hwnds {
        if our == hwnd || unsafe { IsChild(our, hwnd).as_bool() } {
            return false;
        }
    }
    // Check if hwnd is TrayNotifyWnd or any descendant (e.g. ToolbarWindow32, SIBTrayButton, SysPager)
    if let Some(tray) = tray_hwnd {
        if tray == hwnd || unsafe { IsChild(tray, hwnd).as_bool() } {
            return true;
        }
    }
    // Check if hwnd is the taskbar or a child window (e.g. overflow chevron buttons placed directly on Shell_TrayWnd)
    if let Some(taskbar) = taskbar_hwnd {
        if taskbar == hwnd || unsafe { IsChild(taskbar, hwnd).as_bool() } {
            return true;
        }
    }
    false
}

/// WinEvent callback for tray icon location changes
pub(super) unsafe extern "system" fn on_tray_location_changed(
    _hook: HWINEVENTHOOK,
    _event: u32,
    hwnd: HWND,
    _id_object: i32,
    _id_child: i32,
    _thread: u32,
    _time: u32,
) {
    if tray_reposition_is_suppressed() {
        return;
    }

    let (is_tray, our_hwnd) = {
        let state = lock_state();
        let Some(s) = state.as_ref() else {
            return;
        };
        let our_hwnds = std::iter::once(s.hwnd.to_hwnd())
            .chain(s.mirror_hwnds.iter().map(|h| h.to_hwnd()))
            .chain(s.desktop_hwnds.iter().flatten().map(|h| h.to_hwnd()))
            .collect::<Vec<_>>();
        let tray = s.tray_notify_hwnd.map(|h| h.to_hwnd());
        let taskbar = s.taskbar_hwnd.map(|h| h.to_hwnd());
        let is_tray = is_tray_event_source(hwnd, tray, taskbar, &our_hwnds);
        (is_tray, s.hwnd.to_hwnd())
    };

    if !is_tray {
        return;
    }

    schedule_tray_reposition(our_hwnd);
}

/// Called on the window's UI thread by both shell events and watchdog requests.
pub(super) fn schedule_tray_reposition(hwnd: HWND) {
    // Debounce tray location events: shell layout passes (e.g. icon addition,
    // modification like G-Helper NIM_MODIFY, or animation) fire multiple intermediate
    // events. Resetting a short trailing timer ensures we wait for the final settled
    // geometry before updating the position, avoiding transient jumps and jitter.
    const TRAY_REPOSITION_TRAILING_DELAY_MS: u32 = 80;
    unsafe {
        let _ = SetTimer(
            Some(hwnd),
            TIMER_TRAY_REPOSITION,
            TRAY_REPOSITION_TRAILING_DELAY_MS,
            None,
        );
    }
}

pub(super) fn calculate_rect_overlap_ratio(a: RECT, b: RECT) -> f64 {
    let inter_left = a.left.max(b.left);
    let inter_top = a.top.max(b.top);
    let inter_right = a.right.min(b.right);
    let inter_bottom = a.bottom.min(b.bottom);

    if inter_right <= inter_left || inter_bottom <= inter_top {
        return 0.0;
    }

    let inter_area = ((inter_right - inter_left) as f64) * ((inter_bottom - inter_top) as f64);
    let a_area = (((a.right - a.left) as f64) * ((a.bottom - a.top) as f64)).max(1.0);

    inter_area / a_area
}

pub(super) fn should_snap_to_slot(widget: RECT, slot: RECT, was_snapped: bool) -> bool {
    let threshold = if was_snapped { 0.45 } else { 0.67 };
    calculate_rect_overlap_ratio(widget, slot) >= threshold
}

pub(super) struct WidgetFrame {
    pub width: i32,
    pub height: i32,
    pub content_width: i32,
    pub inset: i32,
}

impl WidgetFrame {
    pub fn content_rect(&self, origin: POINT) -> RECT {
        RECT {
            left: origin.x + self.inset,
            top: origin.y,
            right: origin.x + self.inset + self.content_width,
            bottom: origin.y + self.height,
        }
    }
}

pub(super) fn widget_frame(
    theme: &ThemeDocument,
    data: Option<&AppUsageData>,
    runtime: ThemeRuntime,
    scale: f64,
) -> WidgetFrame {
    let (content_width, height) =
        theme_engine::resolve_surface_content_size(theme, 0, data, runtime);
    let inset = theme_engine::surface_horizontal_padding(theme, 0, runtime);
    WidgetFrame {
        width: scaled_theme_dimension(content_width + 2 * inset, scale),
        height: scaled_theme_dimension(height, scale),
        content_width: scaled_theme_dimension(content_width, scale),
        inset: (inset as f64 * scale).round() as i32,
    }
}

pub(super) fn monitor_index_for_handle(
    displays: &[native_interop::DisplayMonitor],
    handle: HMONITOR,
) -> Option<usize> {
    displays.iter().position(|display| display.handle == handle)
}

pub(super) fn override_primary_placement(
    theme: &mut ThemeDocument,
    placement: theme_engine::Placement,
) {
    theme.placement = placement.clone();
    if let Some(surface) = theme.surfaces.first_mut() {
        surface.placement = placement;
    }
}

pub(super) fn floating_placement(display: usize) -> theme_engine::Placement {
    theme_engine::Placement {
        reference: theme_engine::ReferenceTarget {
            region: ReferenceRegion::Monitor,
            display,
        },
        nest: SurfaceNest::Floating,
        horizontal: HorizontalAnchor::Left,
        vertical: VerticalAnchor::Top,
        surface_horizontal: Some(HorizontalAnchor::Left),
        surface_vertical: Some(VerticalAnchor::Top),
        ..Default::default()
    }
}

pub(super) fn dock_placement(
    display: usize,
    offset: i32,
    scale: f64,
    horizontal: bool,
) -> theme_engine::Placement {
    let offset = legacy_offset_to_theme_offset(offset, scale);
    theme_engine::Placement {
        reference: theme_engine::ReferenceTarget {
            region: ReferenceRegion::SystemTray,
            display,
        },
        nest: SurfaceNest::Taskbar,
        horizontal: HorizontalAnchor::Left,
        vertical: if horizontal {
            VerticalAnchor::Bottom
        } else {
            VerticalAnchor::Top
        },
        surface_horizontal: Some(if horizontal {
            HorizontalAnchor::Right
        } else {
            HorizontalAnchor::Left
        }),
        surface_vertical: Some(VerticalAnchor::Bottom),
        offset_x: if horizontal { offset } else { 0 },
        offset_y: if horizontal { 0 } else { offset },
        ..Default::default()
    }
}

pub(super) fn clamped_floating_offset(
    point: POINT,
    monitor: RECT,
    frame: &WidgetFrame,
    scale: f64,
) -> POINT {
    let offset = logical_monitor_offset(point, monitor, scale);
    // Clamp in logical coordinates too, so rounding at fractional DPI cannot
    // put the right or bottom edge back outside the monitor.
    POINT {
        x: offset.x.clamp(
            0,
            (((monitor.right - monitor.left - frame.width).max(0) as f64) / scale).floor() as i32,
        ),
        y: offset.y.clamp(
            0,
            (((monitor.bottom - monitor.top - frame.height).max(0) as f64) / scale).floor() as i32,
        ),
    }
}

pub(super) fn system_tray_reference(taskbar: RECT, tray: Option<RECT>) -> RECT {
    tray.unwrap_or_else(|| {
        if native_interop::is_taskbar_horizontal(taskbar) {
            RECT {
                left: taskbar.right,
                ..taskbar
            }
        } else {
            RECT {
                top: taskbar.bottom,
                ..taskbar
            }
        }
    })
}

pub(super) fn surface_screen_rect(
    placement: &theme_engine::Placement,
    width: i32,
    height: i32,
    scale: f64,
    monitor: RECT,
    taskbar: Option<RECT>,
    tray: Option<RECT>,
) -> RECT {
    let reference = match placement.reference.region {
        ReferenceRegion::Monitor => monitor,
        ReferenceRegion::Taskbar => taskbar.unwrap_or(monitor),
        ReferenceRegion::SystemTray => taskbar
            .map(|tb| system_tray_reference(tb, tray))
            .unwrap_or(monitor),
    };
    let x = aligned_origin(
        reference.left,
        reference.right - reference.left,
        width,
        horizontal_anchor_factor(placement.horizontal),
        horizontal_anchor_factor(placement.surface_horizontal.unwrap_or(placement.horizontal)),
        (placement.offset_x as f64 * scale).round() as i32,
    );
    let y = aligned_origin(
        reference.top,
        reference.bottom - reference.top,
        height,
        vertical_anchor_factor(placement.vertical),
        vertical_anchor_factor(placement.surface_vertical.unwrap_or(placement.vertical)),
        (placement.offset_y as f64 * scale).round() as i32,
    );
    RECT {
        left: x,
        top: y,
        right: x + width,
        bottom: y + height,
    }
}

pub(super) fn auto_eject_origin(widget: RECT, taskbar: RECT, monitor: RECT) -> POINT {
    let width = (widget.right - widget.left).max(1);
    let height = (widget.bottom - widget.top).max(1);
    if native_interop::is_taskbar_horizontal(taskbar) {
        POINT {
            x: widget.left,
            y: if (taskbar.top - monitor.top).abs() <= 50 {
                taskbar.bottom + 6
            } else {
                taskbar.top - height - 6
            },
        }
    } else {
        POINT {
            x: if (taskbar.left - monitor.left).abs() <= 50 {
                taskbar.right + 6
            } else {
                taskbar.left - width - 6
            },
            y: widget.top,
        }
    }
}

pub(super) fn monitor_for_point(
    displays: &[native_interop::DisplayMonitor],
    point: POINT,
) -> (usize, native_interop::DisplayMonitor) {
    displays
        .iter()
        .enumerate()
        .find(|(_, display)| {
            point.x >= display.rect.left
                && point.x < display.rect.right
                && point.y >= display.rect.top
                && point.y < display.rect.bottom
        })
        .or_else(|| {
            displays
                .iter()
                .enumerate()
                .find(|(_, display)| display.primary)
        })
        .or_else(|| displays.iter().enumerate().next())
        .map(|(index, display)| (index, *display))
        .unwrap_or((
            0,
            native_interop::DisplayMonitor {
                handle: HMONITOR::default(),
                rect: RECT {
                    left: 0,
                    top: 0,
                    right: 1920,
                    bottom: 1080,
                },
                primary: true,
            },
        ))
}

pub(super) fn logical_monitor_offset(point: POINT, monitor: RECT, scale: f64) -> POINT {
    POINT {
        x: ((point.x - monitor.left) as f64 / scale).round() as i32,
        y: ((point.y - monitor.top) as f64 / scale).round() as i32,
    }
}

pub(super) fn is_taskbar_capacity_sufficient(
    taskbar_rect: RECT,
    free_dock_slot: RECT,
    widget_width: i32,
    widget_height: i32,
) -> bool {
    let taskbar_w = taskbar_rect.right - taskbar_rect.left;
    let taskbar_h = taskbar_rect.bottom - taskbar_rect.top;
    let slot_w = free_dock_slot.right - free_dock_slot.left;
    let slot_h = free_dock_slot.bottom - free_dock_slot.top;

    if taskbar_w >= taskbar_h {
        // Horizontal taskbar
        slot_w >= widget_width && taskbar_h >= widget_height
    } else {
        // Vertical taskbar: widget doesn't fit inside narrow vertical bar
        taskbar_w >= widget_width && slot_h >= widget_height
    }
}

/// Choose a measured gap near the drag, including gaps left of centred apps.
pub(super) fn taskbar_free_dock_slot(
    taskbar_hwnd: HWND,
    taskbar_rect: RECT,
    widget: RECT,
) -> Option<RECT> {
    let mut lane = widget;
    if native_interop::is_taskbar_horizontal(taskbar_rect) {
        lane.top = compute_anchor_y(
            taskbar_rect.top,
            taskbar_rect.bottom - taskbar_rect.top,
            widget.bottom - widget.top,
        );
        lane.bottom = lane.top + widget.bottom - widget.top;
    } else {
        lane.left = taskbar_rect.left;
        lane.right = lane.left + widget.right - widget.left;
    }
    taskbar_collision::cached(taskbar_hwnd, taskbar_rect)?
        .free_slots(lane)
        .into_iter()
        .filter(|slot| {
            is_taskbar_capacity_sufficient(
                taskbar_rect,
                *slot,
                widget.right - widget.left,
                widget.bottom - widget.top,
            )
        })
        .max_by(|a, b| {
            calculate_rect_overlap_ratio(widget, *a)
                .total_cmp(&calculate_rect_overlap_ratio(widget, *b))
        })
}
