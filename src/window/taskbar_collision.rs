//! Physical taskbar occupancy. Shell accessibility calls stay on a dedicated
//! MTA thread; neither the UI thread nor the Explorer watchdog waits for them.
use super::*;
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoUninitialize, CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED,
};
use windows::Win32::UI::Accessibility::*;

const MAX_SAMPLE_AGE: Duration = Duration::from_secs(6);
static OCCUPANCY: Mutex<Vec<(SendHwnd, Occupancy, Instant)>> = Mutex::new(Vec::new());

#[derive(Clone, Debug)]
pub(super) struct Occupancy {
    pub bounds: RECT,
    pub occupied: Vec<RECT>,
    pub reserved: Vec<RECT>,
}

fn intersection(a: RECT, b: RECT) -> Option<RECT> {
    let rect = RECT {
        left: a.left.max(b.left),
        top: a.top.max(b.top),
        right: a.right.min(b.right),
        bottom: a.bottom.min(b.bottom),
    };
    (rect.right > rect.left && rect.bottom > rect.top).then_some(rect)
}

impl Occupancy {
    pub fn overlaps(&self, widget: RECT) -> bool {
        self.occupied
            .iter()
            .any(|item| intersection(*item, widget).is_some())
    }

    pub fn can_restore(&self, widget: RECT, margin: i32) -> bool {
        if intersection(widget, self.bounds) != Some(widget) {
            return false;
        }
        // Clearance is required on BOTH sides of app buttons, but not against
        // the outside taskbar edges. Keep separate rows independent.
        let mut padded = widget;
        if native_interop::is_taskbar_horizontal(self.bounds) {
            padded.left = padded.left.saturating_sub(margin.max(0));
            padded.right = padded.right.saturating_add(margin.max(0));
        } else {
            padded.top = padded.top.saturating_sub(margin.max(0));
            padded.bottom = padded.bottom.saturating_add(margin.max(0));
        }
        !self.overlaps(widget)
            && !self.occupied.iter().any(|item| {
                // Widgets normally touch the tray edge; require clearance from
                // moving app buttons, not from fixed notification-area controls.
                !self
                    .reserved
                    .iter()
                    .any(|fixed| intersection(*item, *fixed) == Some(*item))
                    && intersection(*item, padded).is_some()
            })
    }

    /// Candidate gaps on either side of centred buttons, or between groups.
    /// Only obstacles in the widget's row/column consume space in that lane.
    pub fn free_slots(&self, widget: RECT) -> Vec<RECT> {
        let horizontal = native_interop::is_taskbar_horizontal(self.bounds);
        let (start, end) = if horizontal {
            (self.bounds.left, self.bounds.right)
        } else {
            (self.bounds.top, self.bounds.bottom)
        };
        let mut intervals: Vec<_> = self
            .occupied
            .iter()
            .filter_map(|item| {
                let in_lane = if horizontal {
                    item.top < widget.bottom && widget.top < item.bottom
                } else {
                    item.left < widget.right && widget.left < item.right
                };
                in_lane.then_some(if horizontal {
                    (item.left, item.right)
                } else {
                    (item.top, item.bottom)
                })
            })
            .collect();
        intervals.sort_unstable();
        let mut cursor = start;
        let mut gaps = Vec::new();
        for (left, right) in intervals.into_iter().chain(std::iter::once((end, end))) {
            let left = left.clamp(start, end);
            let right = right.clamp(start, end);
            if left > cursor {
                let mut rect = self.bounds;
                if horizontal {
                    rect.left = cursor;
                    rect.right = left;
                } else {
                    rect.top = cursor;
                    rect.bottom = left;
                }
                gaps.push(rect);
            }
            cursor = cursor.max(right);
        }
        gaps
    }
}

pub(super) fn cached(hwnd: HWND, bounds: RECT) -> Option<Occupancy> {
    if !unsafe { IsWindowVisible(hwnd).as_bool() } {
        return None;
    }
    let cache = OCCUPANCY.lock().unwrap_or_else(|e| e.into_inner());
    find_sample(&cache, hwnd, bounds, Instant::now())
}

fn find_sample(
    cache: &[(SendHwnd, Occupancy, Instant)],
    hwnd: HWND,
    bounds: RECT,
    now: Instant,
) -> Option<Occupancy> {
    cache
        .iter()
        .find(|(handle, sample, at)| {
            handle.to_hwnd() == hwnd
                && sample.bounds == bounds
                && now.saturating_duration_since(*at) <= MAX_SAMPLE_AGE
        })
        .map(|(_, sample, _)| sample.clone())
}

// Some Win10/third-party shells expose task buttons through legacy UIA proxies.
// Use control roles, not localized titles or Windows-11-only class names.
fn occupies_space(role: UIA_CONTROLTYPE_ID) -> bool {
    [
        UIA_ButtonControlTypeId,
        UIA_SplitButtonControlTypeId,
        UIA_CheckBoxControlTypeId,
        UIA_RadioButtonControlTypeId,
        UIA_ListItemControlTypeId,
        UIA_TabItemControlTypeId,
        UIA_MenuItemControlTypeId,
        UIA_EditControlTypeId,
    ]
    .contains(&role)
}

struct ComApartment;
impl ComApartment {
    fn new() -> windows::core::Result<Self> {
        unsafe {
            CoInitializeEx(None, COINIT_MULTITHREADED).ok()?;
        }
        Ok(Self)
    }
}
impl Drop for ComApartment {
    fn drop(&mut self) {
        unsafe {
            CoUninitialize();
        }
    }
}

struct Reader {
    automation: IUIAutomation2,
    request: IUIAutomationCacheRequest,
    condition: IUIAutomationCondition,
}

impl Reader {
    fn new() -> windows::core::Result<Self> {
        unsafe {
            let automation: IUIAutomation2 =
                CoCreateInstance(&CUIAutomation8, None, CLSCTX_INPROC_SERVER)?;
            automation.SetConnectionTimeout(500)?;
            automation.SetTransactionTimeout(1000)?;
            let request = automation.CreateCacheRequest()?;
            request.SetTreeScope(TreeScope_Element)?;
            request.SetAutomationElementMode(AutomationElementMode_None)?;
            for property in [
                UIA_BoundingRectanglePropertyId,
                UIA_IsOffscreenPropertyId,
                UIA_ControlTypePropertyId,
                UIA_ProcessIdPropertyId,
                UIA_ClassNamePropertyId,
            ] {
                request.AddProperty(property)?;
            }
            let condition = automation.CreateTrueCondition()?;
            Ok(Self {
                automation,
                request,
                condition,
            })
        }
    }

    fn read(
        &self,
        taskbar: native_interop::TaskbarWindow,
    ) -> windows::core::Result<Option<Occupancy>> {
        unsafe {
            let bounds = taskbar.rect;
            if !IsWindowVisible(taskbar.hwnd).as_bool() || !taskbar_shown(bounds) {
                return Ok(None);
            }
            let root = self.automation.ElementFromHandle(taskbar.hwnd)?;
            let elements =
                root.FindAllBuildCache(TreeScope_Descendants, &self.condition, &self.request)?;
            let mut occupied = Vec::new();
            let mut reserved = Vec::new();
            let count = elements.Length()?;
            // An absent accessibility tree is unknown, never proof of free space.
            if count == 0 {
                return Ok(None);
            }
            let mut found_control = false;
            for index in 0..count {
                let element = elements.GetElement(index)?;
                if element.CachedProcessId()? == std::process::id() as i32
                    || !occupies_space(element.CachedControlType()?)
                {
                    continue;
                }
                found_control = true;
                if !element.CachedIsOffscreen()?.as_bool() {
                    if let Some(rect) = intersection(element.CachedBoundingRectangle()?, bounds) {
                        if element
                            .CachedClassName()?
                            .to_string()
                            .starts_with("SystemTray.")
                        {
                            reserved.push(rect);
                        }
                        occupied.push(rect);
                    }
                }
            }
            // A shell with no accessible controls cannot be measured reliably.
            if !found_control {
                return Ok(None);
            }
            // Reserve the notification area even if some of its icons lack UIA.
            if let Some(tray) = native_interop::find_child_window(taskbar.hwnd, "TrayNotifyWnd")
                .and_then(native_interop::get_window_rect_safe)
                .and_then(|rect| intersection(rect, bounds))
            {
                occupied.push(tray);
                reserved.push(tray);
            }
            // Reject a tree captured while the taskbar moved (auto-hide/DPI).
            if native_interop::get_taskbar_rect(taskbar.hwnd) != Some(bounds) {
                return Ok(None);
            }
            Ok(Some(Occupancy {
                bounds,
                occupied,
                reserved,
            }))
        }
    }
}

fn taskbar_shown(bounds: RECT) -> bool {
    let horizontal = native_interop::is_taskbar_horizontal(bounds);
    let thickness = if horizontal {
        bounds.bottom - bounds.top
    } else {
        bounds.right - bounds.left
    };
    thickness > 2
        && native_interop::find_monitors().iter().any(|monitor| {
            intersection(bounds, monitor.rect).is_some_and(|visible| {
                let shown = if horizontal {
                    visible.bottom - visible.top
                } else {
                    visible.right - visible.left
                };
                shown >= thickness
            })
        })
}

pub(super) fn spawn_reader() {
    std::thread::spawn(|| {
        let Ok(_com) = ComApartment::new() else {
            diagnose::log("taskbar occupancy: COM initialization failed");
            return;
        };
        loop {
            match Reader::new() {
                Ok(reader) => loop {
                    let mut next = Vec::new();
                    for taskbar in native_interop::find_taskbars() {
                        match reader.read(taskbar) {
                            Ok(Some(sample)) => next.push((
                                SendHwnd::from_hwnd(taskbar.hwnd),
                                sample,
                                Instant::now(),
                            )),
                            Ok(None) => {}
                            Err(error) => {
                                diagnose::log(format!("taskbar occupancy unavailable: {error}"))
                            }
                        }
                    }
                    // No STATE lock is ever acquired on this thread. Failed or
                    // hidden taskbars disappear from the cache, not become empty.
                    *OCCUPANCY.lock().unwrap_or_else(|e| e.into_inner()) = next;
                    std::thread::sleep(Duration::from_secs(2));
                },
                Err(error) => {
                    diagnose::log(format!("taskbar occupancy reader unavailable: {error}"))
                }
            }
            std::thread::sleep(Duration::from_secs(2));
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(left: i32, top: i32, right: i32, bottom: i32) -> RECT {
        RECT {
            left,
            top,
            right,
            bottom,
        }
    }

    fn layout(bounds: RECT, occupied: Vec<RECT>) -> Occupancy {
        Occupancy {
            bounds,
            occupied,
            reserved: Vec::new(),
        }
    }

    #[test]
    fn measured_windows_11_buttons_detect_the_reported_185_pixel_overlap() {
        let sample = layout(
            rect(0, 1392, 3440, 1440),
            vec![
                rect(55, 1392, 926, 1440), // legacy extent stopped here
                rect(2780, 1392, 2960, 1440),
                rect(2960, 1392, 3140, 1440), // real visible button
            ],
        );
        let widget = rect(2955, 1394, 3201, 1440);
        assert!(sample.overlaps(widget));
        assert!(!sample.can_restore(widget, 20));
        assert_eq!(
            intersection(widget, sample.occupied[2]),
            Some(rect(2960, 1394, 3140, 1440))
        );
    }

    #[test]
    fn centred_buttons_allow_widgets_on_either_side_and_detect_growth_both_ways() {
        let bounds = rect(0, 1032, 1920, 1080);
        let left = rect(400, 1034, 600, 1080);
        let right = rect(1320, 1034, 1520, 1080);
        let sample = layout(bounds, vec![rect(700, 1032, 1220, 1080)]);
        for widget in [left, right] {
            assert!(!sample.overlaps(widget));
            assert!(sample.can_restore(widget, 20));
        }
        let expanded = layout(bounds, vec![rect(580, 1032, 1340, 1080)]);
        for widget in [left, right] {
            assert!(expanded.overlaps(widget));
        }
        let gaps = sample.free_slots(left);
        assert_eq!(
            gaps,
            [rect(0, 1032, 700, 1080), rect(1220, 1032, 1920, 1080)]
        );
    }

    #[test]
    fn return_clearance_applies_to_both_app_edges_but_allows_touching_tray() {
        let bounds = rect(0, 0, 1920, 48);
        let mut sample = layout(
            bounds,
            vec![rect(600, 0, 1200, 48), rect(1700, 0, 1920, 48)],
        );
        sample.reserved.push(rect(1700, 0, 1920, 48));
        for (left, right, expected) in [
            (400, 580, true),
            (400, 581, false),
            (1219, 1400, false),
            (1220, 1400, true),
            (1500, 1700, true),
            (1600, 1800, false),
            (-1, 200, false),
            (1800, 2000, false),
        ] {
            assert_eq!(sample.can_restore(rect(left, 0, right, 48), 20), expected);
        }
    }

    #[test]
    fn orientation_monitor_origin_and_dpi_do_not_change_collision_decisions() {
        for scale in [1, 2, 3] {
            for (x, y) in [(0, 0), (-1920, 0), (0, -1080), (3840, 2160)] {
                for vertical in [false, true] {
                    let transform = |a: i32, b: i32, c: i32, d: i32| {
                        if vertical {
                            rect(x + b * scale, y + a * scale, x + d * scale, y + c * scale)
                        } else {
                            rect(x + a * scale, y + b * scale, x + c * scale, y + d * scale)
                        }
                    };
                    let sample =
                        layout(transform(0, 0, 1920, 48), vec![transform(700, 0, 1220, 48)]);
                    assert!(sample.overlaps(transform(650, 2, 850, 48)));
                    assert!(sample.overlaps(transform(1200, 2, 1400, 48)));
                    assert!(sample.can_restore(transform(400, 2, 600, 48), 20 * scale));
                    assert!(sample.can_restore(transform(1300, 2, 1500, 48), 20 * scale));
                    assert!(!sample.can_restore(transform(1220, 2, 1420, 48), 20 * scale));
                }
            }
        }
    }

    #[test]
    fn multiple_groups_and_rows_use_actual_intersections_not_one_union() {
        let sample = layout(
            rect(0, 0, 1920, 96),
            vec![
                rect(0, 0, 200, 96),
                rect(300, 0, 1000, 48),
                rect(1300, 0, 1500, 96),
            ],
        );
        let lower_row = rect(500, 48, 800, 96);
        assert!(!sample.overlaps(lower_row));
        assert!(sample.can_restore(lower_row, 20));
        assert_eq!(
            sample.free_slots(lower_row),
            [rect(200, 0, 1300, 96), rect(1500, 0, 1920, 96)]
        );
        assert!(sample.overlaps(rect(500, 47, 800, 95)));
        assert!(sample.can_restore(rect(1050, 0, 1250, 48), 20));
    }

    #[test]
    fn overlapping_controls_are_merged_when_selecting_free_gaps() {
        let sample = layout(
            rect(-1920, 0, 0, 48),
            vec![
                rect(-1600, 0, -1200, 48),
                rect(-1700, 0, -1400, 48),
                rect(-1600, 0, -1200, 48),
                rect(-200, 0, 0, 48),
            ],
        );
        assert_eq!(
            sample.free_slots(sample.bounds),
            [rect(-1920, 0, -1700, 48), rect(-1200, 0, -200, 48)]
        );
        let full = layout(sample.bounds, vec![sample.bounds]);
        assert!(full.free_slots(sample.bounds).is_empty());
    }

    #[test]
    fn container_bounds_are_not_treated_as_buttons() {
        for role in [
            UIA_PaneControlTypeId,
            UIA_WindowControlTypeId,
            UIA_ToolBarControlTypeId,
            UIA_ListControlTypeId,
            UIA_TextControlTypeId,
        ] {
            assert!(!occupies_space(role));
        }
        for role in [
            UIA_ButtonControlTypeId,
            UIA_ListItemControlTypeId,
            UIA_EditControlTypeId,
        ] {
            assert!(occupies_space(role));
        }
    }

    #[test]
    fn stale_failed_moved_and_recreated_taskbars_are_unknown_not_free() {
        let now = Instant::now();
        let hwnd = HWND(1usize as *mut _);
        let other = HWND(2usize as *mut _);
        let bounds = rect(0, 1032, 1920, 1080);
        let sample = layout(bounds, vec![rect(700, 1032, 1220, 1080)]);
        let cache = vec![(SendHwnd::from_hwnd(hwnd), sample, now)];
        assert!(find_sample(&cache, hwnd, bounds, now).is_some());
        assert!(find_sample(&cache, other, bounds, now).is_none());
        assert!(find_sample(&cache, hwnd, rect(0, 1079, 1920, 1127), now).is_none());
        assert!(find_sample(
            &cache,
            hwnd,
            bounds,
            now + MAX_SAMPLE_AGE + Duration::from_millis(1)
        )
        .is_none());
        assert!(find_sample(&[], hwnd, bounds, now).is_none());
    }

    #[test]
    #[ignore = "reads the current desktop's taskbar accessibility tree"]
    fn live_taskbar_occupancy() {
        let _com = ComApartment::new().unwrap();
        let reader = Reader::new().unwrap();
        let mut measured = 0;
        for taskbar in native_interop::find_taskbars() {
            if let Some(sample) = reader.read(taskbar).unwrap() {
                measured += 1;
                println!("taskbar={:?} occupied={:?}", sample.bounds, sample.occupied);
                if let Some(widget) =
                    native_interop::find_child_window(taskbar.hwnd, "ClaudeCodeUsageMonitor")
                        .and_then(native_interop::get_window_rect_safe)
                {
                    println!(
                        "widget={widget:?} collision={} restore={}",
                        sample.overlaps(widget),
                        sample.can_restore(widget, 20)
                    );
                }
            }
        }
        assert!(
            measured > 0,
            "no visible accessible taskbar could be measured"
        );
    }
}
