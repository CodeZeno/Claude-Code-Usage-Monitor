use super::*;
use std::cell::RefCell;
use windows::Win32::Foundation::LRESULT;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;

#[test]
fn browser_opener_rejects_non_web_targets_without_launching_them() {
    for target in [
        "",
        "cmd.exe",
        "file:///C:/Windows/System32/cmd.exe",
        "ms-settings:",
        "https://example.com\0cmd.exe",
    ] {
        assert!(!open_web_url(None, target));
    }
}

#[derive(Debug)]
struct TransitionObservation {
    message: u32,
    visible: bool,
    popup: bool,
    rect: (i32, i32, i32, i32),
}

thread_local! {
    static OBSERVATIONS: RefCell<Vec<TransitionObservation>> = const { RefCell::new(Vec::new()) };
}

unsafe extern "system" fn observe_transition(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    let result = DefWindowProcW(hwnd, message, wparam, lparam);
    if matches!(
        message,
        WM_STYLECHANGED | WM_WINDOWPOSCHANGED | WM_SHOWWINDOW
    ) {
        if let Some(rect) = get_window_rect_safe(hwnd) {
            OBSERVATIONS.with(|observations| {
                observations.borrow_mut().push(TransitionObservation {
                    message,
                    visible: IsWindowVisible(hwnd).as_bool(),
                    popup: GetWindowLongW(hwnd, GWL_STYLE) as u32 & WS_CHILD_STYLE == 0,
                    rect: (rect.left, rect.top, rect.right, rect.bottom),
                });
            });
        }
    }
    result
}

struct TestWindow(HWND);

impl Drop for TestWindow {
    fn drop(&mut self) {
        unsafe {
            let _ = DestroyWindow(self.0);
        }
    }
}

#[test]
fn detaching_a_child_never_exposes_parent_relative_coordinates_as_a_popup() {
    unsafe {
        let class = wide_str("UsageMonitorPopupTransitionRegression");
        let instance = GetModuleHandleW(PCWSTR::null()).unwrap();
        let atom = RegisterClassW(&WNDCLASSW {
            lpfnWndProc: Some(observe_transition),
            hInstance: instance.into(),
            lpszClassName: PCWSTR(class.as_ptr()),
            ..Default::default()
        });
        assert_ne!(atom, 0);

        // Transparent, non-activating private windows exercise the real Win32
        // transition without touching Explorer or displaying a test window.
        let parent = TestWindow(
            CreateWindowExW(
                WS_EX_LAYERED | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
                PCWSTR(class.as_ptr()),
                PCWSTR::null(),
                WS_POPUP,
                600,
                900,
                600,
                50,
                None,
                None,
                Some(instance.into()),
                None,
            )
            .unwrap(),
        );
        SetLayeredWindowAttributes(parent.0, Default::default(), 0, LWA_ALPHA).unwrap();
        let _ = ShowWindow(parent.0, SW_SHOWNOACTIVATE);

        for initially_visible in [true, false] {
            let child = TestWindow(
                CreateWindowExW(
                    WS_EX_LAYERED | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
                    PCWSTR(class.as_ptr()),
                    PCWSTR::null(),
                    WS_CHILD,
                    180,
                    2,
                    217,
                    46,
                    Some(parent.0),
                    None,
                    Some(instance.into()),
                    None,
                )
                .unwrap(),
            );
            SetLayeredWindowAttributes(child.0, Default::default(), 0, LWA_ALPHA).unwrap();
            if initially_visible {
                let _ = ShowWindow(child.0, SW_SHOWNOACTIVATE);
            }
            let before = get_window_rect_safe(child.0).unwrap();
            let expected = (before.left, before.top, before.right, before.bottom);
            OBSERVATIONS.with(|observations| observations.borrow_mut().clear());

            make_popup(child.0, true);

            assert!(GetParent(child.0).is_err());
            let after = get_window_rect_safe(child.0).unwrap();
            assert_eq!(
                (after.left, after.top, after.right, after.bottom),
                expected,
                "detaching must preserve the screen rectangle"
            );
            assert_eq!(IsWindowVisible(child.0).as_bool(), initially_visible);
            assert_ne!(
                GetWindowLongW(child.0, GWL_EXSTYLE) as u32 & WS_EX_TOPMOST.0,
                0
            );
            OBSERVATIONS.with(|observations| {
                let observations = observations.borrow();
                assert!(
                    !observations.is_empty(),
                    "the real window procedure must observe the transition"
                );
                for observation in observations.iter().filter(|o| o.visible && o.popup) {
                    assert_eq!(
                        observation.rect, expected,
                        "visible popup moved during message {:x}: {observations:?}",
                        observation.message
                    );
                }
            });

            OBSERVATIONS.with(|observations| observations.borrow_mut().clear());
            make_popup(child.0, false);
            assert_eq!(IsWindowVisible(child.0).as_bool(), initially_visible);
            assert_eq!(
                GetWindowLongW(child.0, GWL_EXSTYLE) as u32 & WS_EX_TOPMOST.0,
                0
            );
            OBSERVATIONS.with(|observations| {
                for observation in observations.borrow().iter() {
                    assert_eq!(
                        observation.visible, initially_visible,
                        "an existing popup must not be hidden and shown again"
                    );
                    assert_eq!(observation.rect, expected);
                }
            });
        }
        drop(parent);
        UnregisterClassW(PCWSTR(class.as_ptr()), Some(instance.into())).unwrap();
    }
}
