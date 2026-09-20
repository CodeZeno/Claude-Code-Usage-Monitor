use super::*;
use std::cell::RefCell;

thread_local! {
    static LAYERED_TRANSITIONS: RefCell<Vec<bool>> = const { RefCell::new(Vec::new()) };
}

unsafe extern "system" fn observe_layered_style(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if message == WM_STYLECHANGED && wparam.0 as i32 == GWL_EXSTYLE.0 {
        let styles = &*(lparam.0 as *const STYLESTRUCT);
        if (styles.styleOld ^ styles.styleNew) & WS_EX_LAYERED.0 != 0 {
            LAYERED_TRANSITIONS.with(|transitions| {
                transitions
                    .borrow_mut()
                    .push(styles.styleNew & WS_EX_LAYERED.0 != 0);
            });
        }
    }
    DefWindowProcW(hwnd, message, wparam, lparam)
}

struct TestWindow(HWND);

impl TestWindow {
    fn new(ex_style: WINDOW_EX_STYLE) -> Self {
        unsafe {
            let class = native_interop::wide_str("UsageMonitorLayeredSurfaceRegression");
            let instance = GetModuleHandleW(PCWSTR::null()).unwrap();
            // Tests use private, hidden windows and never touch Explorer.
            RegisterClassW(&WNDCLASSW {
                lpfnWndProc: Some(observe_layered_style),
                hInstance: instance.into(),
                lpszClassName: PCWSTR(class.as_ptr()),
                ..Default::default()
            });
            Self(
                CreateWindowExW(
                    ex_style | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
                    PCWSTR(class.as_ptr()),
                    PCWSTR::null(),
                    WS_POPUP,
                    0,
                    0,
                    1,
                    1,
                    None,
                    None,
                    Some(instance.into()),
                    None,
                )
                .unwrap(),
            )
        }
    }

    fn render(&self, width: u32) {
        render_custom_window(
            self.0,
            &theme_engine::RenderedTheme {
                width,
                height: 2,
                pixels: vec![0; width as usize * 2],
                warnings: Vec::new(),
            },
            false,
        );
        // UpdateLayeredWindow applies the bitmap size. Changing it on each
        // call also verifies that presentation succeeded after reparenting.
        let rect = native_interop::get_window_rect_safe(self.0).unwrap();
        assert_eq!(rect.right - rect.left, width as i32);
        assert_eq!(rect.bottom - rect.top, 2);
    }
}

impl Drop for TestWindow {
    fn drop(&mut self) {
        unsafe {
            let _ = DestroyWindow(self.0);
        }
    }
}

fn take_transitions() -> Vec<bool> {
    LAYERED_TRANSITIONS.with(|transitions| std::mem::take(&mut *transitions.borrow_mut()))
}

#[test]
fn watchdog_tray_requests_leave_a_delayed_reposition_pending() {
    let window = TestWindow::new(WINDOW_EX_STYLE::default());
    for already_pending in [false, true] {
        if already_pending {
            schedule_tray_reposition(window.0);
        }
        unsafe {
            // Exercise the production message handler without touching Explorer
            // or waiting on wall-clock timing. Both paths must leave a timer.
            wnd_proc(
                window.0,
                native_interop::WM_APP_TRAY_REPOSITION,
                WPARAM(0),
                LPARAM(0),
            );
            assert!(KillTimer(Some(window.0), TIMER_TRAY_REPOSITION).is_ok());
        }
    }
}

#[test]
fn routine_rendering_keeps_the_layered_surface_alive() {
    for ex_style in [WS_EX_LAYERED, WINDOW_EX_STYLE::default()] {
        let window = TestWindow::new(ex_style);
        take_transitions();
        window.render(2);
        assert_eq!(
            take_transitions(),
            if ex_style == WS_EX_LAYERED {
                vec![]
            } else {
                vec![true]
            }
        );
        for width in 3..6 {
            window.render(width);
            assert!(take_transitions().is_empty());
        }
    }
}

#[test]
fn docking_rebinds_layered_surface_only_when_parent_changes() {
    let first_parent = TestWindow::new(WINDOW_EX_STYLE::default());
    let second_parent = TestWindow::new(WINDOW_EX_STYLE::default());
    let window = TestWindow::new(WS_EX_LAYERED);
    window.render(2);
    take_transitions();

    for (index, parent) in [first_parent.0, second_parent.0, first_parent.0]
        .into_iter()
        .enumerate()
    {
        native_interop::embed_as_child(window.0, parent);
        assert_eq!(unsafe { GetParent(window.0) }.unwrap(), parent);
        assert_eq!(take_transitions(), vec![false, true]);
        window.render(3 + index as u32 * 2);
        for _ in 0..3 {
            native_interop::embed_as_child(window.0, parent);
            window.render(4 + index as u32 * 2);
        }
        assert!(take_transitions().is_empty());
    }

    native_interop::make_popup(window.0, true);
    window.render(9);
    assert!(take_transitions().is_empty());
    native_interop::embed_as_child(window.0, first_parent.0);
    assert_eq!(take_transitions(), vec![false, true]);
    window.render(10);
    assert!(take_transitions().is_empty());
}

#[test]
fn reparenting_desktop_composition_windows_does_not_enable_layering() {
    let parent = TestWindow::new(WINDOW_EX_STYLE::default());
    let window = TestWindow::new(WS_EX_NOREDIRECTIONBITMAP);
    take_transitions();
    native_interop::embed_as_child(window.0, parent.0);
    assert_eq!(unsafe { GetParent(window.0) }.unwrap(), parent.0);
    assert!(take_transitions().is_empty());
    let ex_style = unsafe { GetWindowLongW(window.0, GWL_EXSTYLE) } as u32;
    assert_eq!(ex_style & WS_EX_LAYERED.0, 0);
    assert_ne!(ex_style & WS_EX_NOREDIRECTIONBITMAP.0, 0);
}
