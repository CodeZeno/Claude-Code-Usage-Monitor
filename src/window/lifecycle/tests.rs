use super::*;

/// Run the native lifetime regression in its own process: it publishes STATE
/// and the controller handle, while the rest of the suite runs concurrently.
#[test]
fn shell_surface_loss_preserves_controller_timers_and_state() {
    const FIXTURE: &str = "CCUM_TEST_SURFACE_LIFETIME";
    if std::env::var_os(FIXTURE).is_none() {
        let mut child = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "window::lifecycle::tests::shell_surface_loss_preserves_controller_timers_and_state", "--nocapture"])
            .env(FIXTURE, "1")
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn().unwrap();
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            if child.try_wait().unwrap().is_some() {
                break;
            }
            if Instant::now() >= deadline {
                child.kill().unwrap();
                let output = child.wait_with_output().unwrap();
                panic!(
                    "lifetime regression timed out: {}",
                    String::from_utf8_lossy(&output.stderr)
                );
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        let output = child.wait_with_output().unwrap();
        assert!(
            output.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        return;
    }
    unsafe {
        exercise_lifetime();
    }
}

unsafe fn exercise_lifetime() {
    let instance = GetModuleHandleW(PCWSTR::null()).unwrap();
    let class = native_interop::wide_str("UsageMonitorTestController");
    RegisterClassW(&WNDCLASSW {
        lpfnWndProc: Some(controller_wnd_proc),
        hInstance: instance.into(),
        lpszClassName: PCWSTR(class.as_ptr()),
        ..Default::default()
    });
    let controller = CreateWindowExW(
        WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
        PCWSTR(class.as_ptr()),
        PCWSTR::null(),
        WS_POPUP,
        0,
        0,
        0,
        0,
        None,
        None,
        Some(instance.into()),
        None,
    )
    .unwrap();
    CONTROLLER_HWND.store(controller.0 as isize, Ordering::Relaxed);
    assert!(!IsWindowVisible(controller).as_bool());
    assert!(GetParent(controller).is_err());
    assert_eq!(GetWindowLongW(controller, GWL_STYLE) as u32 & WS_CHILD.0, 0);

    let mut theme = ThemeDocument::starter();
    theme.surfaces.truncate(1);
    let mut duplicate = theme.surfaces[0].clone();
    duplicate.id = "mirror".into();
    duplicate.placement.nest = SurfaceNest::Desktop;
    theme.surfaces.push(duplicate);
    let placement = PlacementOverride {
        nest: "taskbar".into(),
        monitor_index: 1,
        monitor_device: Some("missing-display".into()),
        screen_x: 0,
        screen_y: 0,
        tray_offset: 12,
        floating_host: None,
    };
    let mut state = crate::window::placement_regression_tests::state_for(theme, placement.clone());
    state.hwnd = SendHwnd::from_hwnd(create_primary_surface());
    state.data = Some(AppUsageData::default());
    state.retry_count = 7;
    state.last_poll_ok = true;
    state.update_status = UpdateStatus::Checking;
    *lock_state() = Some(state);
    let desktop_host = Cell::new(test_desktop_host());
    let sync = || {
        sync_surface_windows_with(|| {
            let desktop = create_desktop_surface_window_in(native_interop::DesktopHost {
                parent: desktop_host.get(),
                insert_after: HWND_TOP,
            });
            assert!(
                !desktop.is_invalid(),
                "desktop fixture creation failed: {:?}",
                GetLastError()
            );
            desktop
        })
    };
    sync();

    for silent_loss in [false, true] {
        let primary = lock_state().as_ref().unwrap().hwnd.to_hwnd();
        let mirror = lock_state().as_ref().unwrap().mirror_hwnds[0].to_hwnd();
        let host = test_host();
        let desktop = lock_state().as_ref().unwrap().desktop_hwnds[1]
            .unwrap()
            .to_hwnd();
        native_interop::embed_as_child(primary, host);
        native_interop::embed_as_child(mirror, host);
        set_controller_timer(Some(primary), TIMER_POLL, 60_000, None);
        // The transient surface owns no polling timer.
        assert!(KillTimer(Some(primary), TIMER_POLL).is_err());
        if silent_loss {
            // Explorer termination can remove surfaces without WM_DESTROY.
            SetWindowLongPtrW(primary, GWLP_WNDPROC, DefWindowProcW as *const () as isize);
            SetWindowLongPtrW(mirror, GWLP_WNDPROC, DefWindowProcW as *const () as isize);
            SetWindowLongPtrW(desktop, GWLP_WNDPROC, DefWindowProcW as *const () as isize);
        }
        DestroyWindow(host).unwrap();
        DestroyWindow(desktop_host.get()).unwrap();
        assert!(!IsWindow(Some(desktop)).as_bool());
        assert!(!IsWindow(Some(primary)).as_bool());
        assert!(!IsWindow(Some(mirror)).as_bool());
        assert!(IsWindow(Some(controller)).as_bool());
        let mut message = MSG::default();
        assert!(!PeekMessageW(&mut message, None, WM_QUIT, WM_QUIT, PM_REMOVE).as_bool());
        assert!(surfaces_missing());
        assert!(KillTimer(Some(controller), TIMER_POLL).is_ok());
        desktop_host.set(test_desktop_host());
        sync();
        assert!(!surfaces_missing());
        let state = lock_state();
        let state = state.as_ref().unwrap();
        assert_eq!(state.retry_count, 7);
        assert!(state.last_poll_ok);
        assert_eq!(state.data, Some(AppUsageData::default()));
        assert_eq!(state.placement_override, Some(placement.clone()));
        assert!(matches!(state.update_status, UpdateStatus::Checking));
        assert_eq!(controller_for(state.hwnd.to_hwnd()), controller);
    }

    let removed = lock_state().as_ref().unwrap().desktop_hwnds[1]
        .unwrap()
        .to_hwnd();
    lock_state()
        .as_mut()
        .unwrap()
        .active_theme
        .as_mut()
        .unwrap()
        .surfaces
        .truncate(1);
    sync();
    assert!(!IsWindow(Some(removed)).as_bool());

    // Native shell callbacks must queue work while STATE is held. Neither
    // broadcasts nor tray notifications may deadlock trying to resolve a surface.
    let state = lock_state();
    for msg in [WM_DISPLAYCHANGE, WM_SETTINGCHANGE] {
        controller_wnd_proc(controller, msg, WPARAM(0), LPARAM(0));
        let mut queued = MSG::default();
        assert!(PeekMessageW(
            &mut queued,
            Some(controller),
            WM_APP_SHELL_CHANGED,
            WM_APP_SHELL_CHANGED,
            PM_REMOVE
        )
        .as_bool());
        assert_eq!(queued.wParam.0, msg as usize);
    }
    controller_wnd_proc(
        controller,
        WM_APP_TRAY,
        WPARAM(42),
        LPARAM(WM_MOUSEMOVE as isize),
    );
    let mut queued = MSG::default();
    assert!(PeekMessageW(
        &mut queued,
        Some(controller),
        native_interop::WM_APP_TRAY_DISPATCH,
        native_interop::WM_APP_TRAY_DISPATCH,
        PM_REMOVE
    )
    .as_bool());
    assert_eq!(queued.wParam.0, 42);
    drop(state);

    // A surface's explicit Exit also targets the controller.
    let primary = lock_state().as_ref().unwrap().hwnd.to_hwnd();
    wnd_proc(primary, WM_APP_QUIT, WPARAM(0), LPARAM(0));
    assert!(PeekMessageW(&mut queued, Some(controller), WM_CLOSE, WM_CLOSE, PM_REMOVE).as_bool());

    // Private test teardown must not invoke the application's real dashboard
    // shutdown request, which could reach a separately running user instance.
    CONTROLLER_HWND.store(0, Ordering::Relaxed);
    let state = lock_state().take().unwrap();
    destroy_surface(state.hwnd.to_hwnd());
    for mirror in state.mirror_hwnds {
        destroy_surface(mirror.to_hwnd());
    }
    SetWindowLongPtrW(
        controller,
        GWLP_WNDPROC,
        DefWindowProcW as *const () as isize,
    );
    DestroyWindow(controller).unwrap();
    DestroyWindow(desktop_host.get()).unwrap();
}

unsafe fn test_host() -> HWND {
    CreateWindowExW(
        WINDOW_EX_STYLE::default(),
        windows::core::w!("STATIC"),
        PCWSTR::null(),
        WS_POPUP,
        0,
        0,
        1,
        1,
        None,
        None,
        Some(GetModuleHandleW(PCWSTR::null()).unwrap().into()),
        None,
    )
    .unwrap()
}

unsafe fn test_desktop_host() -> HWND {
    let previous = SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_UNAWARE);
    let host = test_host();
    let _ = SetThreadDpiAwarenessContext(previous);
    host
}
