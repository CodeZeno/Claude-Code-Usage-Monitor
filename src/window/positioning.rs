use super::*;

pub(super) fn position_at_taskbar() {
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

pub(super) fn reset_layered_window(hwnd: HWND) {
    unsafe {
        let ex_style = GetWindowLongW(hwnd, GWL_EXSTYLE);
        let _ = SetWindowLongW(hwnd, GWL_EXSTYLE, ex_style & !(WS_EX_LAYERED.0 as i32));
        let _ = SetWindowLongW(hwnd, GWL_EXSTYLE, ex_style | WS_EX_LAYERED.0 as i32);
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
        // SetLayeredWindowAttributes and UpdateLayeredWindow cannot be used on
        // the same layered-style lifetime. Reset it in case this surface was
        // previously hosted on the desktop.
        reset_layered_window(hwnd);
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
    let reference = match theme.placement.reference.region {
        ReferenceRegion::Monitor => display.rect,
        ReferenceRegion::Taskbar => taskbar.map(|taskbar| taskbar.rect).unwrap_or(display.rect),
        ReferenceRegion::SystemTray => taskbar
            .and_then(|taskbar| {
                native_interop::find_child_window(taskbar.hwnd, "TrayNotifyWnd")
                    .and_then(native_interop::get_window_rect_safe)
                    .or(Some(taskbar.rect))
            })
            .unwrap_or(display.rect),
    };
    let width = scaled_theme_dimension(theme.canvas.width.max(1), scale);
    let height = scaled_theme_dimension(theme.canvas.height.max(1), scale);
    let reference_width = reference.right - reference.left;
    let reference_height = reference.bottom - reference.top;
    let surface_horizontal = theme
        .placement
        .surface_horizontal
        .unwrap_or(theme.placement.horizontal);
    let surface_vertical = theme
        .placement
        .surface_vertical
        .unwrap_or(theme.placement.vertical);
    let x = aligned_origin(
        reference.left,
        reference_width,
        width,
        horizontal_anchor_factor(theme.placement.horizontal),
        horizontal_anchor_factor(surface_horizontal),
        (theme.placement.offset_x as f64 * scale).round() as i32,
    );
    let y = aligned_origin(
        reference.top,
        reference_height,
        height,
        vertical_anchor_factor(theme.placement.vertical),
        vertical_anchor_factor(surface_vertical),
        (theme.placement.offset_y as f64 * scale).round() as i32,
    );
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
    static LAST_REPOSITION: Mutex<Option<std::time::Instant>> = Mutex::new(None);

    let is_tray = {
        let state = lock_state();
        state
            .as_ref()
            .and_then(|s| s.tray_notify_hwnd)
            .map(|h| h.to_hwnd() == hwnd)
            .unwrap_or(false)
    };

    if is_tray {
        if tray_reposition_is_suppressed() {
            return;
        }

        let should_reposition = {
            let mut last = LAST_REPOSITION.lock().unwrap_or_else(|e| e.into_inner());
            let now = std::time::Instant::now();
            if last
                .map(|t| now.duration_since(t).as_millis() > 500)
                .unwrap_or(true)
            {
                *last = Some(now);
                true
            } else {
                false
            }
        };
        if should_reposition {
            position_at_taskbar();
            render_layered();
        }
    }
}
