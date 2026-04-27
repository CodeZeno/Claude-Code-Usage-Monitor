use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

use windows::core::PCWSTR;
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::Com::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Accessibility::*;
use windows::Win32::UI::WindowsAndMessaging::*;

use crate::diagnose;
use crate::native_interop;

/// Cache of UIA-derived occupants (XAML-rendered icons that have no HWND).
/// Populated by `spawn_uia_scan` on a worker thread; read at drag-start.
/// Stale by up to one drag — refreshed at startup and after each drag end.
static UIA_CACHE: Mutex<Vec<DebugRect>> = Mutex::new(Vec::new());

/// Prevents overlapping UIA scans when the polling interval is shorter than
/// the scan duration. A spawn called while a worker is still running is a
/// no-op; the in-flight worker's results will land in the cache shortly.
static UIA_SCAN_IN_FLIGHT: AtomicBool = AtomicBool::new(false);

/// Set by `--debug-render` at startup. When true, drag-start additionally
/// shows red-bordered overlays around every detected occupant with its
/// class/name + dimensions, on top of the regular open-region highlights.
pub static DEBUG_RENDER_ENABLED: AtomicBool = AtomicBool::new(false);

const DEBUG_BORDER_WIDTH: i32 = 2;

const OVERLAY_CLASS: &str = "ClaudeCodeUsageMonitorHighlight";

const FILL_ALPHA_DIM: u8 = 32;
const BORDER_ALPHA_DIM: u8 = 110;
const FILL_ALPHA_LIT: u8 = 96;
const BORDER_ALPHA_LIT: u8 = 220;
const BORDER_WIDTH: i32 = 2;

#[derive(Clone, Copy, Debug)]
pub struct HighlightRegion {
    pub rect: RECT,
}

/// One detected occupant of the taskbar — either an HWND we found via
/// enumeration, or a UIA-derived XAML element. `label` carries the
/// element's class/name and is rendered when `--debug-render` is on.
#[derive(Clone, Debug)]
pub struct DebugRect {
    pub rect: RECT,
    pub label: String,
}

pub fn cached_uia_occupants() -> Vec<DebugRect> {
    UIA_CACHE
        .lock()
        .map(|g| g.clone())
        .unwrap_or_default()
}

pub fn register_overlay_class(hinstance: HINSTANCE) {
    unsafe {
        let class_name = native_interop::wide_str(OVERLAY_CLASS);
        let wc = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            style: CS_HREDRAW | CS_VREDRAW,
            lpfnWndProc: Some(overlay_wnd_proc),
            hInstance: hinstance,
            hCursor: LoadCursorW(HINSTANCE::default(), IDC_ARROW).unwrap_or_default(),
            hbrBackground: HBRUSH(std::ptr::null_mut()),
            lpszClassName: PCWSTR::from_raw(class_name.as_ptr()),
            ..Default::default()
        };
        let atom = RegisterClassExW(&wc);
        if atom == 0 {
            diagnose::log("highlight: RegisterClassExW returned 0");
        }
    }
}

#[derive(Clone, Debug)]
struct LeafInfo {
    rect: RECT,
    label: String,
}

/// Enumerate occupied leaf rects (with class-name labels) under the taskbar.
/// HWND-only — XAML-rendered icons must be supplied separately via
/// `cached_uia_occupants`.
pub fn compute_debug_rects(taskbar_hwnd: HWND, exclude_hwnds: &[HWND]) -> Vec<DebugRect> {
    let taskbar_rect = match native_interop::get_taskbar_rect(taskbar_hwnd) {
        Some(r) => r,
        None => return Vec::new(),
    };
    let mut leaves = Vec::new();
    collect_taskbar_occupants(taskbar_hwnd, taskbar_rect, exclude_hwnds, &mut leaves);
    leaves
        .into_iter()
        .filter_map(|leaf| {
            let left = leaf.rect.left.max(taskbar_rect.left);
            let right = leaf.rect.right.min(taskbar_rect.right);
            let top = leaf.rect.top.max(taskbar_rect.top);
            let bottom = leaf.rect.bottom.min(taskbar_rect.bottom);
            if right <= left || bottom <= top {
                return None;
            }
            let w = leaf.rect.right - leaf.rect.left;
            let h = leaf.rect.bottom - leaf.rect.top;
            let label = format!("{} {}x{}", leaf.label, w, h);
            Some(DebugRect {
                rect: RECT { left, top, right, bottom },
                label,
            })
        })
        .collect()
}

/// Find every leaf HWND that visually occupies the taskbar area:
/// - descendants of Shell_TrayWnd (the taskbar's own tree)
/// - top-level siblings (e.g., Win11 `TaskbarFrame`) whose rect lies within
///   the taskbar bounds — these host the Start button and pinned apps on Win11.
fn collect_taskbar_occupants(
    taskbar_hwnd: HWND,
    taskbar_rect: RECT,
    exclude: &[HWND],
    out: &mut Vec<LeafInfo>,
) {
    collect_leaves(taskbar_hwnd, taskbar_rect, exclude, out);

    struct EnumData<'a> {
        taskbar_hwnd: HWND,
        taskbar_rect: RECT,
        exclude: &'a [HWND],
        out: &'a mut Vec<LeafInfo>,
    }

    unsafe extern "system" fn cb(hwnd: HWND, lparam: LPARAM) -> BOOL {
        let data = &mut *(lparam.0 as *mut EnumData);
        if hwnd == data.taskbar_hwnd {
            return TRUE;
        }
        if data.exclude.iter().any(|&e| e == hwnd) {
            return TRUE;
        }
        if !IsWindowVisible(hwnd).as_bool() {
            return TRUE;
        }
        let mut rect = RECT::default();
        if GetWindowRect(hwnd, &mut rect).is_err() {
            return TRUE;
        }
        let slack = 8;
        let inside = rect.left >= data.taskbar_rect.left - slack
            && rect.top >= data.taskbar_rect.top - slack
            && rect.right <= data.taskbar_rect.right + slack
            && rect.bottom <= data.taskbar_rect.bottom + slack;
        if !inside {
            return TRUE;
        }

        if has_visible_child(hwnd) {
            collect_leaves(hwnd, data.taskbar_rect, data.exclude, data.out);
        } else {
            out_push_leaf(data.out, hwnd, rect);
        }
        TRUE
    }

    let mut data = EnumData {
        taskbar_hwnd,
        taskbar_rect,
        exclude,
        out,
    };
    unsafe {
        let _ = EnumWindows(Some(cb), LPARAM(&mut data as *mut _ as isize));
    }
}

fn out_push_leaf(out: &mut Vec<LeafInfo>, hwnd: HWND, rect: RECT) {
    if rect.right > rect.left && rect.bottom > rect.top {
        out.push(LeafInfo {
            rect,
            label: get_class_name(hwnd),
        });
    }
}

fn collect_leaves(parent: HWND, taskbar_rect: RECT, exclude: &[HWND], out: &mut Vec<LeafInfo>) {
    unsafe {
        let mut hwnd = HWND::default();
        loop {
            hwnd = match FindWindowExW(parent, hwnd, PCWSTR::null(), PCWSTR::null()) {
                Ok(h) if h != HWND::default() => h,
                _ => break,
            };
            if exclude.iter().any(|&e| e == hwnd) {
                continue;
            }

            let mut rect = RECT::default();
            if GetWindowRect(hwnd, &mut rect).is_err() {
                continue;
            }
            if rect.right <= rect.left || rect.bottom <= rect.top {
                continue;
            }

            let visible = IsWindowVisible(hwnd).as_bool();
            // Win11 keeps legacy HWNDs (e.g. `Start`) marked as invisible while
            // their position still indicates where the XAML-rendered control
            // sits. Trust the rect when it's well-formed and inside the taskbar.
            let rect_inside_taskbar = rect.left >= taskbar_rect.left
                && rect.right <= taskbar_rect.right
                && rect.top >= taskbar_rect.top
                && rect.bottom <= taskbar_rect.bottom;
            if !visible && !rect_inside_taskbar {
                continue;
            }

            let class = get_class_name(hwnd);

            // XAML hosts on Win11 (DesktopWindowContentBridge etc.) contribute
            // no useful HWND descendants. They're enumerated separately on a
            // background thread via UI Automation; skip them here.
            if is_xaml_host(&class) {
                continue;
            }

            // Only descend into actually-visible containers; invisible HWNDs
            // can have stale or empty subtrees.
            if visible && has_visible_child(hwnd) {
                collect_leaves(hwnd, taskbar_rect, exclude, out);
            } else {
                out.push(LeafInfo { rect, label: class });
            }
        }
    }
}

fn is_xaml_host(class: &str) -> bool {
    class == "Windows.UI.Composition.DesktopWindowContentBridge"
        || class.starts_with("Microsoft.UI.Content.")
        || class == "Microsoft.UI.Composition.SwapChainPanel"
}

/// Refresh the UIA cache on a fresh MTA worker. Fire-and-forget — when
/// it completes (typically <1s on Win11), `UIA_CACHE` holds the latest leaves
/// for the next drag to consume. Never modifies overlays directly, so it
/// can't disrupt an in-progress drag.
pub fn spawn_uia_scan(taskbar_hwnd: HWND, taskbar_rect: RECT) {
    if UIA_SCAN_IN_FLIGHT.swap(true, Ordering::AcqRel) {
        return;
    }
    let taskbar_addr = taskbar_hwnd.0 as isize;
    std::thread::spawn(move || {
        let taskbar_hwnd = HWND(taskbar_addr as *mut _);

        let mut raw_leaves: Vec<LeafInfo> = Vec::new();
        unsafe {
            if CoInitializeEx(None, COINIT_MULTITHREADED).is_err() {
                UIA_SCAN_IN_FLIGHT.store(false, Ordering::Release);
                return;
            }
            let mut hosts = Vec::new();
            find_xaml_hosts(taskbar_hwnd, &mut hosts);
            for host in hosts {
                uia_walk_inner(host, taskbar_rect, &mut raw_leaves);
            }
            CoUninitialize();
        }

        let leaves: Vec<DebugRect> = raw_leaves
            .into_iter()
            .filter_map(|leaf| {
                let l = leaf.rect.left.max(taskbar_rect.left);
                let r = leaf.rect.right.min(taskbar_rect.right);
                let t = leaf.rect.top.max(taskbar_rect.top);
                let b = leaf.rect.bottom.min(taskbar_rect.bottom);
                if r <= l || b <= t {
                    return None;
                }
                let w = leaf.rect.right - leaf.rect.left;
                let h = leaf.rect.bottom - leaf.rect.top;
                Some(DebugRect {
                    rect: RECT {
                        left: l,
                        top: t,
                        right: r,
                        bottom: b,
                    },
                    label: format!("{} {}x{}", leaf.label, w, h),
                })
            })
            .collect();

        if let Ok(mut cache) = UIA_CACHE.lock() {
            *cache = leaves;
        }
        UIA_SCAN_IN_FLIGHT.store(false, Ordering::Release);
    });
}

fn find_xaml_hosts(parent: HWND, out: &mut Vec<HWND>) {
    unsafe {
        let mut hwnd = HWND::default();
        loop {
            hwnd = match FindWindowExW(parent, hwnd, PCWSTR::null(), PCWSTR::null()) {
                Ok(h) if h != HWND::default() => h,
                _ => break,
            };
            if !IsWindowVisible(hwnd).as_bool() {
                continue;
            }
            let class = get_class_name(hwnd);
            if is_xaml_host(&class) {
                out.push(hwnd);
            } else if has_visible_child(hwnd) {
                find_xaml_hosts(hwnd, out);
            }
        }
    }
}

/// Compute open regions from an arbitrary list of occupied debug rects.
/// Used to recompute regions when async UIA results arrive.
pub fn open_regions_from_occupants(
    taskbar_rect: RECT,
    occupants: &[DebugRect],
) -> Vec<HighlightRegion> {
    let mut intervals: Vec<(i32, i32)> = occupants
        .iter()
        .filter_map(|d| {
            let l = d.rect.left.max(taskbar_rect.left);
            let r = d.rect.right.min(taskbar_rect.right);
            if r > l { Some((l, r)) } else { None }
        })
        .collect();

    intervals.sort_by_key(|&(l, _)| l);
    let mut merged: Vec<(i32, i32)> = Vec::new();
    for (l, r) in intervals {
        if let Some(last) = merged.last_mut() {
            if l <= last.1 {
                last.1 = last.1.max(r);
                continue;
            }
        }
        merged.push((l, r));
    }

    let mut regions = Vec::new();
    let mut cursor = taskbar_rect.left;
    for (l, r) in &merged {
        if *l > cursor {
            regions.push(HighlightRegion {
                rect: RECT {
                    left: cursor,
                    top: taskbar_rect.top,
                    right: *l,
                    bottom: taskbar_rect.bottom,
                },
            });
        }
        cursor = cursor.max(*r);
    }
    if cursor < taskbar_rect.right {
        regions.push(HighlightRegion {
            rect: RECT {
                left: cursor,
                top: taskbar_rect.top,
                right: taskbar_rect.right,
                bottom: taskbar_rect.bottom,
            },
        });
    }
    regions
}

unsafe fn uia_walk_inner(host: HWND, taskbar_rect: RECT, out: &mut Vec<LeafInfo>) {
    let uia: IUIAutomation = match CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER) {
        Ok(u) => u,
        Err(error) => {
            diagnose::log_error("uia: CoCreateInstance failed", error);
            return;
        }
    };

    let root = match uia.ElementFromHandle(host) {
        Ok(e) => e,
        Err(error) => {
            diagnose::log_error("uia: ElementFromHandle failed", error);
            return;
        }
    };

    // Batch the whole subtree into a single RPC instead of per-node.
    let cache = match uia.CreateCacheRequest() {
        Ok(c) => c,
        Err(error) => {
            diagnose::log_error("uia: CreateCacheRequest failed", error);
            return;
        }
    };
    let _ = cache.AddProperty(UIA_BoundingRectanglePropertyId);
    let _ = cache.AddProperty(UIA_NamePropertyId);
    let _ = cache.AddProperty(UIA_ClassNamePropertyId);

    let condition = match uia.CreateTrueCondition() {
        Ok(c) => c,
        Err(error) => {
            diagnose::log_error("uia: CreateTrueCondition failed", error);
            return;
        }
    };

    let elements = match root.FindAllBuildCache(TreeScope_Descendants, &condition, &cache) {
        Ok(a) => a,
        Err(error) => {
            diagnose::log_error("uia: FindAllBuildCache failed", error);
            return;
        }
    };

    let len = elements.Length().unwrap_or(0);
    let tb_w = taskbar_rect.right - taskbar_rect.left;
    for i in 0..len {
        let elem = match elements.GetElement(i) {
            Ok(e) => e,
            Err(_) => continue,
        };

        let rect = match elem.CachedBoundingRectangle() {
            Ok(r) => r,
            Err(_) => continue,
        };
        if rect.right <= rect.left || rect.bottom <= rect.top {
            continue;
        }
        if rect.right <= taskbar_rect.left
            || rect.left >= taskbar_rect.right
            || rect.bottom <= taskbar_rect.top
            || rect.top >= taskbar_rect.bottom
        {
            continue;
        }
        // Skip elements whose rect is essentially the entire taskbar — those
        // are containers; their children already cover the actual content.
        if rect.right - rect.left >= tb_w * 9 / 10 {
            continue;
        }

        let label = elem
            .CachedName()
            .ok()
            .map(|b| b.to_string())
            .filter(|s| !s.is_empty())
            .or_else(|| elem.CachedClassName().ok().map(|b| b.to_string()))
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "uia".into());

        out.push(LeafInfo { rect, label });
    }
}

fn has_visible_child(parent: HWND) -> bool {
    unsafe {
        let mut hwnd = HWND::default();
        loop {
            hwnd = match FindWindowExW(parent, hwnd, PCWSTR::null(), PCWSTR::null()) {
                Ok(h) if h != HWND::default() => h,
                _ => return false,
            };
            if IsWindowVisible(hwnd).as_bool() {
                return true;
            }
        }
    }
}

fn get_class_name(hwnd: HWND) -> String {
    unsafe {
        let mut buf = [0u16; 256];
        let len = GetClassNameW(hwnd, &mut buf);
        if len > 0 {
            String::from_utf16_lossy(&buf[..len as usize])
        } else {
            String::new()
        }
    }
}

/// Create a layered overlay window covering `rect` (screen coords), parented
/// into Shell_TrayWnd. Caller is responsible for painting + storing the HWND.
fn create_overlay(taskbar_hwnd: HWND, rect: &RECT) -> Option<HWND> {
    let w = rect.right - rect.left;
    let h = rect.bottom - rect.top;
    if w <= 0 || h <= 0 {
        return None;
    }

    unsafe {
        let hinstance = HINSTANCE(GetModuleHandleW(PCWSTR::null()).ok()?.0);
        let class_name = native_interop::wide_str(OVERLAY_CLASS);
        let title = native_interop::wide_str("");

        let hwnd = match CreateWindowExW(
            WS_EX_TOOLWINDOW | WS_EX_LAYERED | WS_EX_NOACTIVATE,
            PCWSTR::from_raw(class_name.as_ptr()),
            PCWSTR::from_raw(title.as_ptr()),
            WS_POPUP,
            rect.left,
            rect.top,
            w,
            h,
            HWND::default(),
            HMENU::default(),
            hinstance,
            None,
        ) {
            Ok(h) => h,
            Err(error) => {
                diagnose::log_error("highlight: CreateWindowExW failed", error);
                return None;
            }
        };

        native_interop::embed_in_taskbar(hwnd, taskbar_hwnd);

        if let Some(taskbar_rect) = native_interop::get_taskbar_rect(taskbar_hwnd) {
            native_interop::move_window(
                hwnd,
                rect.left - taskbar_rect.left,
                rect.top - taskbar_rect.top,
                w,
                h,
            );
        }

        let _ = SetWindowPos(
            hwnd,
            HWND_TOP,
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
        );

        let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);

        Some(hwnd)
    }
}

pub fn show_highlights(taskbar_hwnd: HWND, regions: &[HighlightRegion]) -> Vec<HWND> {
    let mut hwnds = Vec::with_capacity(regions.len());
    for region in regions {
        if let Some(hwnd) = create_overlay(taskbar_hwnd, &region.rect) {
            let w = region.rect.right - region.rect.left;
            let h = region.rect.bottom - region.rect.top;
            paint_white_highlight(hwnd, w, h, false);
            hwnds.push(hwnd);
        }
    }
    hwnds
}

/// Re-render an existing highlight overlay with the dim or lit alpha pair.
/// Re-uses the same `UpdateLayeredWindow` primitive — no window churn.
pub fn repaint_highlight(hwnd: HWND, region: &HighlightRegion, lit: bool) {
    let w = region.rect.right - region.rect.left;
    let h = region.rect.bottom - region.rect.top;
    paint_white_highlight(hwnd, w, h, lit);
}

/// Diagnostic: create an overlay per occupant rect with a red border + label.
/// Caller appends the returned HWNDs into the same vec used for white
/// highlights so they tear down together at drag-end.
pub fn show_debug_rects(taskbar_hwnd: HWND, rects: &[DebugRect]) -> Vec<HWND> {
    let mut hwnds = Vec::with_capacity(rects.len());
    for r in rects {
        if let Some(hwnd) = create_overlay(taskbar_hwnd, &r.rect) {
            let w = r.rect.right - r.rect.left;
            let h = r.rect.bottom - r.rect.top;
            paint_debug_rect(hwnd, w, h, &r.label);
            hwnds.push(hwnd);
        }
    }
    hwnds
}

pub fn hide_highlights(hwnds: &mut Vec<HWND>) {
    unsafe {
        for h in hwnds.drain(..) {
            let _ = DestroyWindow(h);
        }
    }
}

fn paint_white_highlight(hwnd: HWND, width: i32, height: i32, lit: bool) {
    paint_with(hwnd, width, height, move |pixel_data, w, h| {
        let fill_alpha = if lit { FILL_ALPHA_LIT } else { FILL_ALPHA_DIM };
        let border_alpha = if lit { BORDER_ALPHA_LIT } else { BORDER_ALPHA_DIM };
        let fill = u32::from(fill_alpha) * 0x0101_0101;
        let border = u32::from(border_alpha) * 0x0101_0101;
        let bw = BORDER_WIDTH.min(w / 2).min(h / 2).max(0);
        for y in 0..h {
            let row = &mut pixel_data[(y * w) as usize..((y + 1) * w) as usize];
            let on_border_row = y < bw || y >= h - bw;
            for (x, px) in row.iter_mut().enumerate() {
                let on_border =
                    on_border_row || (x as i32) < bw || (x as i32) >= w - bw;
                *px = if on_border { border } else { fill };
            }
        }
    });
}

fn paint_debug_rect(hwnd: HWND, width: i32, height: i32, label: &str) {
    if width <= 0 || height <= 0 {
        return;
    }
    unsafe {
        let screen_dc = GetDC(hwnd);
        let bmi = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: width,
                biHeight: -height,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: 0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut bits: *mut std::ffi::c_void = std::ptr::null_mut();
        let mem_dc = CreateCompatibleDC(screen_dc);
        let dib = CreateDIBSection(mem_dc, &bmi, DIB_RGB_COLORS, &mut bits, None, 0)
            .unwrap_or_default();
        if dib.is_invalid() || bits.is_null() {
            let _ = DeleteDC(mem_dc);
            ReleaseDC(hwnd, screen_dc);
            return;
        }
        let old_bmp = SelectObject(mem_dc, dib);
        let pixel_count = (width * height) as usize;
        let pixel_data = std::slice::from_raw_parts_mut(bits as *mut u32, pixel_count);

        // Render label via GDI on a still-zeroed DIB. Anything GDI touches
        // becomes opaque red after the post-process pass below.
        let _ = SetBkMode(mem_dc, TRANSPARENT);
        let _ = SetTextColor(mem_dc, COLORREF(0x0000_00FF)); // BGR red

        let font_name = native_interop::wide_str("Segoe UI");
        let font = CreateFontW(
            -11,
            0,
            0,
            0,
            FW_BOLD.0 as i32,
            0,
            0,
            0,
            DEFAULT_CHARSET.0 as u32,
            OUT_TT_PRECIS.0 as u32,
            CLIP_DEFAULT_PRECIS.0 as u32,
            NONANTIALIASED_QUALITY.0 as u32,
            (DEFAULT_PITCH.0 | FF_DONTCARE.0) as u32,
            PCWSTR::from_raw(font_name.as_ptr()),
        );
        let old_font = SelectObject(mem_dc, font);

        let mut text_wide: Vec<u16> = label.encode_utf16().collect();
        let mut text_rect = RECT {
            left: DEBUG_BORDER_WIDTH + 2,
            top: DEBUG_BORDER_WIDTH + 1,
            right: width - DEBUG_BORDER_WIDTH - 2,
            bottom: height - DEBUG_BORDER_WIDTH - 1,
        };
        let _ = DrawTextW(
            mem_dc,
            &mut text_wide,
            &mut text_rect,
            DT_LEFT | DT_TOP | DT_SINGLELINE | DT_NOPREFIX,
        );

        SelectObject(mem_dc, old_font);
        let _ = DeleteObject(font);

        // Anything GDI touched becomes opaque (alpha = 255). Untouched pixels
        // stay transparent.
        for px in pixel_data.iter_mut() {
            let rgb = *px & 0x00FF_FFFF;
            if rgb != 0 {
                *px = 0xFF00_0000 | rgb;
            }
        }

        // Stamp the red border directly with premultiplied red.
        let red = 0xFFFF_0000u32;
        let bw = DEBUG_BORDER_WIDTH.min(width / 2).min(height / 2).max(1);
        for y in 0..height {
            let row = &mut pixel_data[(y * width) as usize..((y + 1) * width) as usize];
            let on_border_row = y < bw || y >= height - bw;
            for (x, px) in row.iter_mut().enumerate() {
                let on_border =
                    on_border_row || (x as i32) < bw || (x as i32) >= width - bw;
                if on_border {
                    *px = red;
                }
            }
        }

        let pt_src = POINT { x: 0, y: 0 };
        let sz = SIZE { cx: width, cy: height };
        let blend = BLENDFUNCTION {
            BlendOp: 0,
            BlendFlags: 0,
            SourceConstantAlpha: 255,
            AlphaFormat: 1,
        };
        let _ = UpdateLayeredWindow(
            hwnd,
            screen_dc,
            None,
            Some(&sz),
            mem_dc,
            Some(&pt_src),
            COLORREF(0),
            Some(&blend),
            ULW_ALPHA,
        );

        SelectObject(mem_dc, old_bmp);
        let _ = DeleteObject(dib);
        let _ = DeleteDC(mem_dc);
        ReleaseDC(hwnd, screen_dc);
    }
}

fn paint_with<F>(hwnd: HWND, width: i32, height: i32, fill: F)
where
    F: FnOnce(&mut [u32], i32, i32),
{
    if width <= 0 || height <= 0 {
        return;
    }
    unsafe {
        let screen_dc = GetDC(hwnd);
        let bmi = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: width,
                biHeight: -height,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: 0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut bits: *mut std::ffi::c_void = std::ptr::null_mut();
        let mem_dc = CreateCompatibleDC(screen_dc);
        let dib = CreateDIBSection(mem_dc, &bmi, DIB_RGB_COLORS, &mut bits, None, 0)
            .unwrap_or_default();
        if dib.is_invalid() || bits.is_null() {
            let _ = DeleteDC(mem_dc);
            ReleaseDC(hwnd, screen_dc);
            return;
        }
        let old_bmp = SelectObject(mem_dc, dib);
        let pixel_count = (width * height) as usize;
        let pixel_data = std::slice::from_raw_parts_mut(bits as *mut u32, pixel_count);
        fill(pixel_data, width, height);

        let pt_src = POINT { x: 0, y: 0 };
        let sz = SIZE { cx: width, cy: height };
        let blend = BLENDFUNCTION {
            BlendOp: 0,
            BlendFlags: 0,
            SourceConstantAlpha: 255,
            AlphaFormat: 1,
        };
        let _ = UpdateLayeredWindow(
            hwnd,
            screen_dc,
            None,
            Some(&sz),
            mem_dc,
            Some(&pt_src),
            COLORREF(0),
            Some(&blend),
            ULW_ALPHA,
        );
        SelectObject(mem_dc, old_bmp);
        let _ = DeleteObject(dib);
        let _ = DeleteDC(mem_dc);
        ReleaseDC(hwnd, screen_dc);
    }
}

unsafe extern "system" fn overlay_wnd_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    DefWindowProcW(hwnd, msg, wparam, lparam)
}
