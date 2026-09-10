use super::*;

#[derive(Clone, Copy)]
struct ThemeHostGeometry {
    monitor: RECT,
    taskbar: Option<RECT>,
    scale: f64,
}

// Store values rather than native handles so readers need no Win32 calls.
// Readers may hold STATE; writers must never acquire STATE while holding this lock.
static THEME_HOST_GEOMETRY: Mutex<Vec<ThemeHostGeometry>> = Mutex::new(Vec::new());
static REFRESHING: AtomicBool = AtomicBool::new(false);

/// Called at startup and on shell/display layout changes, with STATE unlocked.
pub(super) fn refresh_theme_host_geometry() {
    refresh_with(|| {
        let displays = native_interop::find_monitors();
        let taskbars = native_interop::find_taskbars();
        displays
            .into_iter()
            .map(|display| ThemeHostGeometry {
                monitor: display.rect,
                taskbar: taskbars
                    .iter()
                    .find(|taskbar| unsafe {
                        MonitorFromWindow(taskbar.hwnd, MONITOR_DEFAULTTOPRIMARY) == display.handle
                    })
                    .map(|taskbar| taskbar.rect),
                scale: monitor_scale(display),
            })
            .collect()
    });
}

fn refresh_with(query: impl FnOnce() -> Vec<ThemeHostGeometry>) {
    // SHAppBarMessage can dispatch another layout notification before returning.
    // Reentrant readers keep using the last complete snapshot; never wait here.
    if REFRESHING.swap(true, Ordering::Acquire) {
        return;
    }
    struct Reset;
    impl Drop for Reset {
        fn drop(&mut self) {
            REFRESHING.store(false, Ordering::Release);
        }
    }
    let _reset = Reset;
    let geometry = query();
    *THEME_HOST_GEOMETRY
        .lock()
        .unwrap_or_else(|e| e.into_inner()) = geometry;
}

/// Safe under STATE: sizing, hit testing and menu evaluation use only cached values.
pub(super) fn theme_runtime_for_surface(
    theme: &ThemeDocument,
    surface_index: usize,
    runtime: ThemeRuntime,
) -> ThemeRuntime {
    let geometry = THEME_HOST_GEOMETRY
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    runtime_with_geometry(theme, surface_index, runtime, &geometry)
}

fn runtime_with_geometry(
    theme: &ThemeDocument,
    surface_index: usize,
    runtime: ThemeRuntime,
    geometry: &[ThemeHostGeometry],
) -> ThemeRuntime {
    let Some(surface) = theme.surfaces.get(surface_index) else {
        return runtime;
    };
    let Some(host) = geometry
        .get(surface.placement.reference.display)
        .or_else(|| geometry.first())
    else {
        return runtime;
    };
    let nest = surface
        .placement
        .nest
        .resolve(surface.placement.reference.region);
    let rect = if matches!(nest, SurfaceNest::Taskbar | SurfaceNest::TrayIcon) {
        host.taskbar.unwrap_or(host.monitor)
    } else {
        host.monitor
    };
    runtime.with_host_dimensions(
        logical_host_dimension(rect.right - rect.left, host.scale),
        logical_host_dimension(rect.bottom - rect.top, host.scale),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn display() -> ThemeHostGeometry {
        ThemeHostGeometry {
            monitor: RECT {
                left: 0,
                top: 0,
                right: 3840,
                bottom: 2160,
            },
            taskbar: Some(RECT {
                left: 0,
                top: 2068,
                right: 3840,
                bottom: 2160,
            }),
            scale: 2.0,
        }
    }

    #[test]
    fn cached_hosts_preserve_nesting_and_dpi() {
        let mut theme = ThemeDocument::starter();
        let runtime = ThemeRuntime::default().with_poll_state(false, true);
        for (nest, height) in [
            (SurfaceNest::Taskbar, 46),
            (SurfaceNest::TrayIcon, 46),
            (SurfaceNest::Floating, 1080),
            (SurfaceNest::Desktop, 1080),
        ] {
            theme.surfaces[0].placement.nest = nest;
            assert_eq!(
                runtime_with_geometry(&theme, 0, runtime, &[display()]),
                runtime.with_host_dimensions(1920, height)
            );
        }
    }

    #[test]
    fn selected_monitor_uses_its_own_scale_and_vertical_taskbar() {
        let mut theme = ThemeDocument::starter();
        theme.surfaces[0].placement.nest = SurfaceNest::TrayIcon;
        theme.surfaces[0].placement.reference.display = 1;
        let mut secondary = display();
        secondary.scale = 1.0;
        secondary.taskbar = Some(RECT {
            left: -1920,
            top: 0,
            right: -1860,
            bottom: 1080,
        });
        let runtime = ThemeRuntime::default();
        assert_eq!(
            runtime_with_geometry(&theme, 0, runtime, &[display(), secondary]),
            runtime.with_host_dimensions(60, 1080)
        );
    }

    #[test]
    fn missing_display_and_taskbar_keep_existing_fallbacks() {
        let mut theme = ThemeDocument::starter();
        theme.surfaces[0].placement.nest = SurfaceNest::TrayIcon;
        theme.surfaces[0].placement.reference.display = 99;
        let runtime = ThemeRuntime::default();
        let mut host = display();
        host.taskbar = None;
        assert_eq!(
            runtime_with_geometry(&theme, 0, runtime, &[host]),
            runtime.with_host_dimensions(1920, 1080)
        );
        assert_eq!(runtime_with_geometry(&theme, 0, runtime, &[]), runtime);
        assert_eq!(runtime_with_geometry(&theme, 99, runtime, &[host]), runtime);
    }

    #[test]
    fn shell_query_can_reenter_state_and_read_previous_geometry() {
        let theme = ThemeDocument::starter();
        let runtime = ThemeRuntime::default();
        refresh_with(|| vec![display()]);
        let previous = theme_runtime_for_surface(&theme, 0, runtime);
        refresh_with(|| {
            // Simulate a sent message delivered during the shell round-trip.
            let _state = lock_state();
            assert_eq!(theme_runtime_for_surface(&theme, 0, runtime), previous);
            refresh_with(|| panic!("a reentrant layout message must not query the shell"));
            let mut host = display();
            host.scale = 1.0;
            vec![host]
        });
        assert_eq!(
            theme_runtime_for_surface(&theme, 0, runtime),
            runtime.with_host_dimensions(3840, 92)
        );
    }
}
