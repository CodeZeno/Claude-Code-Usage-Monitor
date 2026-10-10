//! Hide-until-hover presentation for floating surfaces docked to a monitor's
//! top or bottom edge.
//!
//! A collapsed surface shows only a slim handle, which is click-through so it
//! never blocks whatever is underneath. Click-through windows receive no mouse
//! messages, so the pointer is polled on a window timer: a few microseconds per
//! tick, unlike a global low-level mouse hook that would sit in the input path
//! of every application. The full surface is rendered once per data change and
//! slid in by presenting a growing slice of that bitmap, so animation frames
//! never re-run the theme renderer.

use super::*;

/// The pointer must rest on the handle this long, so a cursor flying past
/// along the screen edge does not open the surface.
pub(super) const INTENT_DELAY: Duration = Duration::from_millis(80);
/// How long the pointer may be away before the surface slides back.
pub(super) const LEAVE_DELAY: Duration = Duration::from_millis(600);
pub(super) const REVEAL_DURATION: Duration = Duration::from_millis(250);
pub(super) const HIDE_DURATION: Duration = Duration::from_millis(200);
const FRAME_INTERVAL_MS: u32 = 16;
pub(super) const POLL_INTERVAL_MS: u32 = 40;
/// Logical size of the pointer target around the handle.
const ZONE_WIDTH: f64 = 200.0;
const ZONE_DEPTH: f64 = 6.0;
/// Logical slack around an open surface before the pointer counts as gone.
const LEAVE_SLACK: f64 = 6.0;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct Pointer {
    pub over_handle: bool,
    pub over_surface: bool,
    pub button_down: bool,
    /// The surface's `force_reveal` expression holds it open, wherever the
    /// pointer is.
    pub force_reveal: bool,
}

/// Reveal state machine. `progress` is linear time from 0 (collapsed) to 1
/// (open); `eased` turns it into the visible fraction.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(super) struct Reveal {
    pub open: bool,
    pub progress: f64,
    hover_since: Option<Instant>,
    away_since: Option<Instant>,
    last_tick: Option<Instant>,
}

impl Reveal {
    pub fn tick(&mut self, now: Instant, pointer: Pointer) {
        let was_open = self.open;
        if !self.open {
            if pointer.force_reveal {
                self.open = true;
                self.hover_since = None;
                self.away_since = None;
            // A held button means a window is being dragged to the edge.
            } else if pointer.over_handle && !pointer.button_down {
                let since = *self.hover_since.get_or_insert(now);
                if now.duration_since(since) >= INTENT_DELAY {
                    self.open = true;
                    self.hover_since = None;
                    self.away_since = None;
                }
            } else {
                self.hover_since = None;
            }
        } else if pointer.force_reveal || pointer.over_surface || pointer.over_handle {
            self.away_since = None;
        } else {
            let since = *self.away_since.get_or_insert(now);
            if now.duration_since(since) >= LEAVE_DELAY {
                self.open = false;
                self.away_since = None;
            }
        }
        // A slide starts on the tick that decides it; time before that
        // decision must not make the first frame jump.
        let elapsed = self
            .last_tick
            .filter(|_| was_open == self.open)
            .map_or(0.0, |last| now.duration_since(last).as_secs_f64());
        self.last_tick = Some(now);
        self.progress = if self.open {
            (self.progress + elapsed / REVEAL_DURATION.as_secs_f64()).min(1.0)
        } else {
            (self.progress - elapsed / HIDE_DURATION.as_secs_f64()).max(0.0)
        };
    }

    /// Visible fraction: decelerates into place when opening and accelerates
    /// away when closing.
    pub fn eased(&self) -> f64 {
        1.0 - (1.0 - self.progress.clamp(0.0, 1.0)).powi(3)
    }

    /// Whether the next tick should come at animation rate.
    pub fn busy(&self) -> bool {
        (self.open && self.progress < 1.0)
            || (!self.open && self.progress > 0.0)
            || self.hover_since.is_some()
    }
}

/// Physical screen geometry of one auto-hiding surface.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Layout {
    pub edge: theme_engine::SurfaceEdge,
    /// The fully revealed surface.
    pub surface: RECT,
    /// The collapsed tab.
    pub handle: RECT,
    /// Where resting the pointer reveals the surface.
    pub zone: RECT,
}

pub(super) fn layout(
    surface: RECT,
    edge: theme_engine::SurfaceEdge,
    handle: (u32, u32),
    scale: f64,
) -> Layout {
    let width = surface.right - surface.left;
    let center = surface.left + width / 2;
    let span = |length: i32| {
        let length = length.clamp(1, width.max(1));
        let left = center - length / 2;
        (left, left + length)
    };
    let at_edge = |depth: i32, (left, right): (i32, i32)| match edge {
        theme_engine::SurfaceEdge::Top => RECT {
            left,
            top: surface.top,
            right,
            bottom: surface.top + depth,
        },
        theme_engine::SurfaceEdge::Bottom => RECT {
            left,
            top: surface.bottom - depth,
            right,
            bottom: surface.bottom,
        },
    };
    Layout {
        edge,
        surface,
        handle: at_edge(handle.1 as i32, span(handle.0 as i32)),
        zone: at_edge(
            (ZONE_DEPTH * scale).round().max(1.0) as i32,
            span((ZONE_WIDTH * scale).round() as i32),
        ),
    }
}

/// What one presentation shows: the handle, or a slice of the surface that
/// is anchored to the edge and grows away from it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Frame {
    pub window: RECT,
    pub handle: bool,
    /// First bitmap row shown when presenting a surface slice.
    pub source_top: i32,
    pub opacity: u8,
    /// Only a fully open surface takes clicks; everything else passes them on.
    pub interactive: bool,
}

pub(super) fn frame(layout: &Layout, eased: f64) -> Frame {
    let height = layout.surface.bottom - layout.surface.top;
    let visible = ((eased.clamp(0.0, 1.0) * height as f64).round() as i32).min(height);
    if visible <= 0 {
        return Frame {
            window: layout.handle,
            handle: true,
            source_top: 0,
            opacity: 255,
            interactive: false,
        };
    }
    let (top, source_top) = match layout.edge {
        theme_engine::SurfaceEdge::Top => (layout.surface.top, height - visible),
        theme_engine::SurfaceEdge::Bottom => (layout.surface.bottom - visible, 0),
    };
    Frame {
        window: RECT {
            top,
            bottom: top + visible,
            ..layout.surface
        },
        handle: false,
        source_top,
        opacity: ((0.35 + 0.65 * eased.clamp(0.0, 1.0)) * 255.0).round() as u8,
        interactive: visible == height,
    }
}

pub(super) fn pointer(layout: &Layout, cursor: POINT, button_down: bool, scale: f64) -> Pointer {
    let contains = |rect: RECT| {
        cursor.x >= rect.left
            && cursor.x < rect.right
            && cursor.y >= rect.top
            && cursor.y < rect.bottom
    };
    let slack = (LEAVE_SLACK * scale).round() as i32;
    Pointer {
        over_handle: contains(layout.zone),
        over_surface: contains(RECT {
            left: layout.surface.left - slack,
            top: layout.surface.top - slack,
            right: layout.surface.right + slack,
            bottom: layout.surface.bottom + slack,
        }),
        button_down,
        force_reveal: false,
    }
}

struct Surface {
    hwnd: SendHwnd,
    layout: Layout,
    scale: f64,
    image: std::sync::Arc<theme_engine::RenderedTheme>,
    handle: std::sync::Arc<theme_engine::RenderedTheme>,
    /// The theme holds the surface open, for example during an alarm.
    force_reveal: bool,
    reveal: Reveal,
    shown: Option<Frame>,
}

// Never call Win32 while holding this lock: style and position changes can
// synchronously re-enter the window procedure.
static SURFACES: Mutex<Vec<Surface>> = Mutex::new(Vec::new());
/// Keeps open surfaces open while a context menu they launched is showing.
static HOLD_OPEN: AtomicBool = AtomicBool::new(false);

fn surfaces() -> MutexGuard<'static, Vec<Surface>> {
    SURFACES.lock().unwrap_or_else(|error| error.into_inner())
}

pub(super) fn owns(hwnd: HWND) -> bool {
    surfaces()
        .iter()
        .any(|surface| surface.hwnd.to_hwnd() == hwnd)
}

pub(super) fn hold_open(hold: bool) {
    HOLD_OPEN.store(hold, Ordering::Release);
}

/// Install freshly rendered pixels and geometry for an auto-hiding surface,
/// keeping its reveal state, and present it in that state. A forced surface
/// slides open on the next tick and stays open until it is no longer forced.
pub(super) fn present(
    hwnd: HWND,
    layout: Layout,
    scale: f64,
    image: theme_engine::RenderedTheme,
    handle: theme_engine::RenderedTheme,
    force_reveal: bool,
) {
    let (image, handle) = (std::sync::Arc::new(image), std::sync::Arc::new(handle));
    let frame = {
        let mut surfaces = surfaces();
        let reveal = surfaces
            .iter()
            .position(|surface| surface.hwnd.to_hwnd() == hwnd)
            .map(|index| surfaces.swap_remove(index).reveal)
            .unwrap_or_default();
        let frame = frame(&layout, reveal.eased());
        surfaces.push(Surface {
            hwnd: SendHwnd::from_hwnd(hwnd),
            layout,
            scale,
            image: image.clone(),
            handle: handle.clone(),
            force_reveal,
            reveal,
            shown: Some(frame),
        });
        frame
    };
    apply(hwnd, &image, &handle, frame);
}

/// Forget surfaces that are no longer auto-hiding and restore their input.
pub(super) fn retain(active: &[HWND]) {
    let removed = {
        let mut surfaces = surfaces();
        let mut removed = Vec::new();
        surfaces.retain(|surface| {
            let keep = active.contains(&surface.hwnd.to_hwnd());
            if !keep {
                removed.push(surface.hwnd);
            }
            keep
        });
        removed
    };
    for hwnd in removed {
        set_click_through(hwnd.to_hwnd(), false);
    }
}

/// How often the pointer poll should run, or `None` when no surface can
/// react to the pointer: every auto-hiding surface is hidden (for example
/// behind a fullscreen app) and already collapsed. Whatever shows one again
/// calls `schedule`.
pub(super) fn poll_interval() -> Option<u32> {
    let (windows, busy, dirty) = {
        let surfaces = surfaces();
        (
            surfaces
                .iter()
                .map(|surface| surface.hwnd.to_hwnd())
                .collect::<Vec<_>>(),
            surfaces.iter().any(|surface| surface.reveal.busy()),
            // A hidden surface that is not collapsed yet needs one more tick.
            surfaces
                .iter()
                .any(|surface| surface.reveal != Reveal::default()),
        )
    };
    let visible = windows.into_iter().any(is_shown);
    if !visible && !dirty {
        return None;
    }
    Some(if busy {
        FRAME_INTERVAL_MS
    } else {
        POLL_INTERVAL_MS
    })
}

/// Run or stop the pointer poll that drives every auto-hiding surface.
pub(super) fn schedule(owner: HWND) {
    unsafe {
        match poll_interval() {
            Some(interval) => {
                SetTimer(Some(owner), TIMER_AUTO_HIDE, interval, None);
            }
            None => {
                let _ = KillTimer(Some(owner), TIMER_AUTO_HIDE);
            }
        }
    }
}

/// Whether the poll timer is armed, which stops it: for tests that follow
/// the real timer rather than the interval `schedule` would pick.
#[cfg(test)]
pub(super) fn take_timer(owner: HWND) -> bool {
    unsafe { KillTimer(Some(owner), TIMER_AUTO_HIDE).is_ok() }
}

/// The pointer's screen position, when known, and whether the primary button
/// is held.
#[cfg(not(test))]
fn pointer_state() -> (Option<POINT>, bool) {
    use windows::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, VK_LBUTTON, VK_RBUTTON};
    let mut cursor = POINT::default();
    let cursor_known = unsafe { GetCursorPos(&mut cursor) }.is_ok();
    let button_down = unsafe {
        // GetAsyncKeyState reads physical buttons; follow a swapped mouse.
        let primary = if GetSystemMetrics(SM_SWAPBUTTON) != 0 {
            VK_RBUTTON
        } else {
            VK_LBUTTON
        };
        GetAsyncKeyState(primary.0 as i32) as u16 & 0x8000 != 0
    };
    (cursor_known.then_some(cursor), button_down)
}

#[cfg(not(test))]
pub(super) fn is_shown(hwnd: HWND) -> bool {
    unsafe { IsWindowVisible(hwnd).as_bool() }
}

/// Show or hide an auto-hiding surface's window.
#[cfg(not(test))]
pub(super) fn show(hwnd: HWND, show: bool) {
    unsafe {
        let _ = ShowWindow(hwnd, if show { SW_SHOWNOACTIVATE } else { SW_HIDE });
    }
}

// Tests stand in for the desktop: surfaces are only marked shown, never put on
// screen, and the pointer is nowhere.
#[cfg(test)]
thread_local! {
    static TEST_SHOWN: std::cell::RefCell<Vec<isize>> = const { std::cell::RefCell::new(Vec::new()) };
}

#[cfg(test)]
fn pointer_state() -> (Option<POINT>, bool) {
    (None, false)
}

#[cfg(test)]
pub(super) fn is_shown(hwnd: HWND) -> bool {
    TEST_SHOWN.with(|shown| shown.borrow().contains(&(hwnd.0 as isize)))
}

#[cfg(test)]
pub(super) fn show(hwnd: HWND, show: bool) {
    TEST_SHOWN.with(|shown| {
        let mut shown = shown.borrow_mut();
        shown.retain(|window| *window != hwnd.0 as isize);
        if show {
            shown.push(hwnd.0 as isize);
        }
    });
}

pub(super) fn tick(owner: HWND) {
    let (cursor, button_down) = pointer_state();
    let held = HOLD_OPEN.load(Ordering::Acquire);
    let now = Instant::now();
    let windows: Vec<_> = surfaces()
        .iter()
        .map(|surface| surface.hwnd.to_hwnd())
        .collect();
    // Hidden surfaces (for example behind a fullscreen app) stay collapsed.
    let visible: Vec<_> = windows.iter().map(|hwnd| is_shown(*hwnd)).collect();
    let updates = {
        let mut surfaces = surfaces();
        let mut updates = Vec::new();
        for surface in surfaces.iter_mut() {
            let hwnd = surface.hwnd.to_hwnd();
            let shown = windows
                .iter()
                .position(|window| *window == hwnd)
                .is_some_and(|index| visible[index]);
            if shown {
                let mut input = cursor.map_or_else(Pointer::default, |cursor| {
                    pointer(&surface.layout, cursor, button_down, surface.scale)
                });
                input.over_surface |= held && surface.reveal.open;
                input.force_reveal = surface.force_reveal;
                surface.reveal.tick(now, input);
            } else {
                // Collapse at once so the surface never reappears open.
                surface.reveal = Reveal::default();
            }
            let frame = frame(&surface.layout, surface.reveal.eased());
            if surface.shown != Some(frame) {
                surface.shown = Some(frame);
                updates.push((hwnd, surface.image.clone(), surface.handle.clone(), frame));
            }
        }
        updates
    };
    for (hwnd, image, handle, frame) in updates {
        apply(hwnd, &image, &handle, frame);
    }
    schedule(owner);
}

fn apply(
    hwnd: HWND,
    image: &theme_engine::RenderedTheme,
    handle: &theme_engine::RenderedTheme,
    frame: Frame,
) {
    set_click_through(hwnd, !frame.interactive);
    let origin = POINT {
        x: frame.window.left,
        y: frame.window.top,
    };
    let (bitmap, source_top) = if frame.handle {
        (handle, 0)
    } else {
        (image, frame.source_top)
    };
    positioning::present_layered(
        hwnd,
        bitmap,
        positioning::LayeredFrame {
            source_top,
            height: frame.window.bottom - frame.window.top,
            origin: Some(origin),
            opacity: frame.opacity,
            interactive: frame.interactive,
        },
    );
}

fn set_click_through(hwnd: HWND, click_through: bool) {
    unsafe {
        let style = GetWindowLongW(hwnd, GWL_EXSTYLE);
        let wanted = if click_through {
            style | WS_EX_TRANSPARENT.0 as i32
        } else {
            style & !(WS_EX_TRANSPARENT.0 as i32)
        };
        if wanted != style {
            let _ = SetWindowLongW(hwnd, GWL_EXSTYLE, wanted);
        }
    }
}
