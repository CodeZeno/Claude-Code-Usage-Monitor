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
    // Layout queries can dispatch another layout notification before returning.
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
    let mut geometry = query();
    let mut cached = THEME_HOST_GEOMETRY
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    retain_taskbar_geometry(&mut geometry, &cached);
    *cached = geometry;
}

fn retain_taskbar_geometry(geometry: &mut [ThemeHostGeometry], previous: &[ThemeHostGeometry]) {
    for host in geometry {
        // Auto-hide may report only the reveal strip, and Explorer can briefly
        // remove the taskbar. Neither should become a new layout host size.
        if host.taskbar.is_none_or(|rect| {
            logical_host_dimension(
                (rect.right - rect.left).min(rect.bottom - rect.top),
                host.scale,
            ) <= 2
        }) {
            host.taskbar = previous
                .iter()
                .find(|old| old.monitor == host.monitor && old.scale == host.scale)
                .and_then(|old| old.taskbar);
        }
    }
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

pub(super) fn taskbar_is_horizontal(display_index: usize) -> bool {
    THEME_HOST_GEOMETRY
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(display_index)
        .and_then(|host| host.taskbar)
        .is_none_or(native_interop::is_taskbar_horizontal)
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
    let nest = surface
        .placement
        .nest
        .resolve(surface.placement.reference.region);
    let runtime = runtime.with_nest(nest);
    if nest == SurfaceNest::Floating {
        if let Some((width, height)) = surface.placement.host_dimensions {
            return runtime.with_host_dimensions(width, height);
        }
    }
    let Some(host) = geometry
        .get(surface.placement.reference.display)
        .or_else(|| geometry.first())
    else {
        return runtime;
    };
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

/// Capture layout inputs, not the resolved window size (which may include chrome
/// or expressions such as host.height / 2). The theme/root identity scopes saved
/// sizes across restarts and theme changes.
pub(super) fn floating_host_for_theme(
    theme: &ThemeDocument,
    saved: Option<&app_settings::FloatingHost>,
) -> Option<app_settings::FloatingHost> {
    let geometry = THEME_HOST_GEOMETRY
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    floating_host_with_geometry(theme, saved, &geometry)
}

fn floating_host_with_geometry(
    theme: &ThemeDocument,
    saved: Option<&app_settings::FloatingHost>,
    geometry: &[ThemeHostGeometry],
) -> Option<app_settings::FloatingHost> {
    let surface = theme.surfaces.first()?;
    if let Some(saved) = saved.filter(|saved| {
        saved.theme_id == theme.id
            && saved.surface_id == surface.id
            && saved.width > 0
            && saved.height > 0
    }) {
        return Some(saved.clone());
    }
    let nest = surface
        .placement
        .nest
        .resolve(surface.placement.reference.region);
    let (width, height) = if let Some(size) = surface.placement.host_dimensions {
        size
    } else if nest == SurfaceNest::Taskbar {
        taskbar_host_dimensions(surface.placement.reference.display, geometry)
    } else {
        return None;
    };
    Some(app_settings::FloatingHost {
        theme_id: theme.id.clone(),
        surface_id: surface.id.clone(),
        width,
        height,
    })
}

fn taskbar_host_dimensions(display: usize, geometry: &[ThemeHostGeometry]) -> (u32, u32) {
    geometry
        .get(display)
        .or_else(|| geometry.first())
        .and_then(|host| {
            host.taskbar.map(|rect| {
                (
                    logical_host_dimension(rect.right - rect.left, host.scale),
                    logical_host_dimension(rect.bottom - rect.top, host.scale),
                )
            })
        })
        // No taskbar is available on startup: use the normal compact fallback,
        // rather than accidentally substituting the monitor's full dimensions.
        .unwrap_or_else(|| ThemeRuntime::default().host_dimensions())
}

pub(super) fn floating_host_for_state(state: &AppState) -> Option<app_settings::FloatingHost> {
    floating_host_for_theme(&effective_theme_from_state(state)?, None)
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
                runtime.with_nest(nest).with_host_dimensions(1920, height)
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
            runtime
                .with_nest(SurfaceNest::TrayIcon)
                .with_host_dimensions(60, 1080)
        );
    }

    #[test]
    fn undocking_preserves_root_children_rendering_and_hit_testing() {
        use theme_engine::*;
        let mut theme = ThemeDocument::starter();
        let surface = &mut theme.surfaces[0];
        surface.width = 292.0.into();
        surface.height = Expression("host.height".into());
        surface.background = LayerBackground::None;
        surface.content = SceneContent::None;
        let mut container = SceneObject::object("usage", "Usage Window");
        container.width = 100.0.into();
        container.height = Expression("host.height".into());
        let mut label = SceneObject::object("remaining", "Remaining");
        label.parent = Some(container.id.clone());
        label.width = 100.0.into();
        label.height = 20.0.into();
        label.anchor.vertical = ObjectVerticalAnchor::Bottom;
        label.mouse_events = Some(MouseEvents {
            click: "refresh()".into(),
            ..Default::default()
        });
        surface.children = vec![container, label];
        let source = floating_host_with_geometry(&theme, None, &[display()]).unwrap();
        assert_eq!((source.width, source.height), (1920, 46));
        let mut placement = positioning::floating_placement(0);
        placement.host_dimensions = Some((source.width, source.height));
        positioning::override_primary_placement(&mut theme, placement);
        // A different monitor (or no monitor snapshot) cannot change layout.
        for geometry in [
            vec![],
            vec![ThemeHostGeometry {
                scale: 1.0,
                ..display()
            }],
        ] {
            let runtime = runtime_with_geometry(&theme, 0, ThemeRuntime::default(), &geometry);
            assert_eq!(resolve_surface_size(&theme, 0, None, runtime), (312, 46));
            assert_eq!(
                resolve_object_bounds_with_runtime(&theme, 0, 1, None, runtime),
                Some((10.0, 26.0, 100.0, 20.0))
            );
            assert_eq!(
                hit_test_mouse_event(&theme, 0, 20.0, 40.0, None, runtime).as_deref(),
                Some("remaining")
            );
            for scale in [1.0, 1.25, 2.0] {
                let frame = positioning::widget_frame(&theme, None, runtime, scale);
                let rendered =
                    render_theme_surface_with_runtime_at_scale(&theme, 0, None, runtime, scale);
                assert_eq!(
                    (rendered.width as i32, rendered.height as i32),
                    (frame.width, frame.height)
                );
                assert_eq!(frame.height, (46.0 * scale).round() as i32);
                assert!(rendered.warnings.is_empty(), "{:?}", rendered.warnings);
            }
        }
        // Dragging again captures neither card padding nor an evaluated size.
        assert_eq!(floating_host_with_geometry(&theme, None, &[]), Some(source));
        theme.surfaces[0].height = Expression("host.height / 2".into());
        let runtime = runtime_with_geometry(&theme, 0, ThemeRuntime::default(), &[]);
        assert_eq!(resolve_surface_size(&theme, 0, None, runtime), (312, 23));
    }

    #[test]
    fn saved_host_is_scoped_and_redocking_adopts_the_new_taskbar() {
        let mut theme = ThemeDocument::starter();
        let source = floating_host_with_geometry(&theme, None, &[display()]).unwrap();
        assert_eq!(
            floating_host_with_geometry(&theme, Some(&source), &[]),
            Some(source.clone())
        );
        theme.surfaces[0].placement = positioning::floating_placement(0);
        // A surface authored as floating still uses the monitor when it has no
        // matching saved undocking context. Other roots are unaffected too.
        assert!(floating_host_with_geometry(&theme, None, &[display()]).is_none());
        let mut foreign = source.clone();
        foreign.theme_id.push_str("-other");
        assert!(floating_host_with_geometry(&theme, Some(&foreign), &[display()]).is_none());
        foreign = source.clone();
        foreign.surface_id.push_str("-other");
        assert!(floating_host_with_geometry(&theme, Some(&foreign), &[display()]).is_none());
        assert_eq!(
            runtime_with_geometry(&theme, 0, ThemeRuntime::default(), &[display()])
                .host_dimensions(),
            (1920, 1080)
        );
        theme.surfaces[0].placement.host_dimensions = Some((source.width, source.height));
        assert_eq!(
            runtime_with_geometry(&theme, 1, ThemeRuntime::default(), &[display()])
                .host_dimensions(),
            (1920, 46)
        );
        positioning::override_primary_placement(
            &mut theme,
            positioning::dock_placement(0, 0, 1.0, false),
        );
        let vertical = ThemeHostGeometry {
            taskbar: Some(RECT {
                left: 0,
                top: 0,
                right: 60,
                bottom: 2160,
            }),
            scale: 1.0,
            ..display()
        };
        let next = floating_host_with_geometry(&theme, None, &[vertical]).unwrap();
        assert_eq!((next.width, next.height), (60, 2160));
        assert_eq!(
            runtime_with_geometry(&theme, 0, ThemeRuntime::default(), &[vertical])
                .host_dimensions(),
            (60, 2160)
        );
        let serialized = serde_json::to_string(&theme).unwrap();
        assert!(!serialized.contains("host_dimensions"));
    }

    #[test]
    fn hidden_or_missing_taskbar_retains_the_last_valid_size() {
        let previous = display();
        for taskbar in [
            None,
            Some(RECT {
                top: 2158,
                ..previous.taskbar.unwrap()
            }),
        ] {
            let mut geometry = [ThemeHostGeometry {
                taskbar,
                ..previous
            }];
            retain_taskbar_geometry(&mut geometry, &[previous]);
            assert_eq!(taskbar_host_dimensions(0, &geometry), (1920, 46));
            retain_taskbar_geometry(&mut geometry, &[]);
            assert_eq!(taskbar_host_dimensions(99, &geometry), (1920, 46));
        }
        let mut geometry = [ThemeHostGeometry {
            taskbar: None,
            ..previous
        }];
        retain_taskbar_geometry(&mut geometry, &[]);
        assert_eq!(
            taskbar_host_dimensions(0, &geometry),
            ThemeRuntime::default().host_dimensions()
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
            runtime
                .with_nest(SurfaceNest::TrayIcon)
                .with_host_dimensions(1920, 1080)
        );
        assert_eq!(
            runtime_with_geometry(&theme, 0, runtime, &[]),
            runtime.with_nest(SurfaceNest::TrayIcon)
        );
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
