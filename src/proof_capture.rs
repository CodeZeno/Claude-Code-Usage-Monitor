use std::fs;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant};

use serde::Serialize;
use windows::Win32::Foundation::{HWND, LPARAM, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::Graphics::Gdi::HGDIOBJ;
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetClassNameW, GetSystemMetrics, GetWindowRect, IsWindowVisible, PostMessageW,
    SYSTEM_METRICS_INDEX,
};

use crate::diagnose;
use crate::native_interop::{self, WM_APP_REQUEST_PROOF};

const DEFAULT_FLAG: &str =
    r"C:\Users\oyardena\PycharmProjects\Claude-Code-Usage-Monitor\.cr-tmp\request-proof.flag";

#[derive(Serialize)]
struct ProofResult {
    pass: bool,
    on_taskbar: bool,
    hwnd: isize,
    hwnd_rect: String,
    layered: String,
    widget_region: String,
    taskbar_band: String,
    fullscreen_rect: String,
    buffer_nonblack: u32,
    desktop_nonblack: u32,
    buffer_hash: u64,
    desktop_hash: u64,
    buffer_matches_desktop: bool,
    pixel_match_pct: u32,
    buffer_len: usize,
    desktop_len: usize,
    visible: bool,
    files: ProofFiles,
}

#[derive(Serialize)]
struct ProofFiles {
    buffer_bmp: String,
    desktop_bmp: String,
    fullscreen_bmp: String,
    manifest: String,
}

pub fn flag_pending() -> bool {
    PathBuf::from(DEFAULT_FLAG).exists()
}

pub fn handle_cli(args: &[String]) -> Option<i32> {
    if args.len() == 3 && args[1] == "--request-proof" {
        let out = PathBuf::from(&args[2]);
        return Some(match request_proof_blocking(&out, Duration::from_secs(45)) {
            Ok(pass) => {
                if pass { 0 } else { 1 }
            }
            Err(error) => {
                diagnose::log(format!("request-proof failed: {error}"));
                1
            }
        });
    }
    None
}

fn notify_running_instance_proof() {
    unsafe {
        let class = native_interop::wide_str("ClaudeCodeUsageMonitor");
        let mut target = HWND::default();
        unsafe extern "system" fn enum_proc(hwnd: HWND, lparam: LPARAM) -> windows::Win32::Foundation::BOOL {
            let target = &mut *(lparam.0 as *mut HWND);
            let mut buf = [0u16; 64];
            let len = GetClassNameW(hwnd, &mut buf);
            if len > 0 {
                let name = String::from_utf16_lossy(&buf[..len as usize]);
                if name == "ClaudeCodeUsageMonitor" {
                    *target = hwnd;
                    return windows::Win32::Foundation::BOOL(0);
                }
            }
            windows::Win32::Foundation::BOOL(1)
        }
        let _ = EnumWindows(Some(enum_proc), LPARAM(&mut target as *mut _ as isize));
        if !target.0.is_null() {
            let _ = PostMessageW(target, WM_APP_REQUEST_PROOF, WPARAM(0), LPARAM(0));
        }
    }
}

pub fn request_proof_blocking(out_dir: &Path, timeout: Duration) -> Result<bool, String> {
    fs::create_dir_all(out_dir).map_err(|e| e.to_string())?;
    fs::write(DEFAULT_FLAG, out_dir.to_string_lossy().as_bytes()).map_err(|e| e.to_string())?;
    notify_running_instance_proof();
    let manifest = out_dir.join("proof.json");
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if manifest.exists() {
            let text = fs::read_to_string(&manifest).map_err(|e| e.to_string())?;
            let value: serde_json::Value =
                serde_json::from_str(&text).map_err(|e| e.to_string())?;
            return Ok(value
                .get("pass")
                .and_then(|v| v.as_bool())
                .unwrap_or(false));
        }
        thread::sleep(Duration::from_millis(250));
    }
    Err("timed out waiting for proof.json from running instance".to_string())
}

pub fn maybe_capture(
    hwnd: HWND,
    taskbar_hwnd: Option<HWND>,
    layered_x: i32,
    layered_y: i32,
    width: i32,
    height: i32,
    buffer: &[u32],
) {
    let flag = PathBuf::from(DEFAULT_FLAG);
    if !flag.exists() {
        return;
    }
    let out_dir = match fs::read_to_string(&flag) {
        Ok(text) => PathBuf::from(text.trim()),
        Err(error) => {
            diagnose::log(format!("proof flag read failed: {error}"));
            let _ = fs::remove_file(flag);
            return;
        }
    };
    let _ = fs::create_dir_all(&out_dir);
    let buffer_path = out_dir.join("proof-buffer.bmp");
    let desktop_path = out_dir.join("proof-desktop.bmp");
    let fullscreen_path = out_dir.join("proof-fullscreen.bmp");
    let manifest_path = out_dir.join("proof.json");

    let buffer_hash = hash_pixels(buffer);
    let buffer_nonblack = count_nonblack_bgra(buffer);
    if let Err(error) = save_bmp32(&buffer_path, width, height, buffer) {
        diagnose::log(format!("proof buffer save failed: {error}"));
    }

    let (cap_x, cap_y, cap_w, cap_h) = (layered_x, layered_y, width, height);
    let desktop = match capture_desktop_bgra(cap_x, cap_y, cap_w, cap_h) {
        Ok(pixels) => pixels,
        Err(error) => {
            diagnose::log(format!("proof desktop capture failed: {error}"));
            Vec::new()
        }
    };
    let desktop_hash = hash_pixels(&desktop);
    let desktop_nonblack = count_nonblack_bgra(&desktop);
    if !desktop.is_empty() {
        if let Err(error) = save_bmp32(&desktop_path, cap_w, cap_h, &desktop) {
            diagnose::log(format!("proof desktop save failed: {error}"));
        }
    }

    let (fs_x, fs_y, fs_w, fs_h) = virtual_screen_bounds();
    if let Ok(fullscreen) = capture_stitched_fullscreen(fs_x, fs_y, fs_w, fs_h) {
        if let Err(error) = save_bmp32(&fullscreen_path, fs_w, fs_h, &fullscreen) {
            diagnose::log(format!("proof fullscreen save failed: {error}"));
        }
    } else {
        diagnose::log("proof stitched fullscreen capture failed".to_string());
    }

    let widget_rect = RECT {
        left: layered_x,
        top: layered_y,
        right: layered_x + width,
        bottom: layered_y + height,
    };
    let (taskbar_band, on_taskbar) = match taskbar_hwnd {
        Some(taskbar_hwnd) => {
            let fallback = native_interop::get_taskbar_rect(taskbar_hwnd).unwrap_or_default();
            let band = native_interop::screen_taskbar_rect(taskbar_hwnd, fallback);
            let on_taskbar = native_interop::widget_on_taskbar_band(taskbar_hwnd, widget_rect);
            (
                format!(
                    "{},{},{},{}",
                    band.left, band.top, band.right, band.bottom
                ),
                on_taskbar,
            )
        }
        None => (String::new(), false),
    };

    let mut hwnd_rect = RECT::default();
    let visible = unsafe {
        GetWindowRect(hwnd, &mut hwnd_rect).is_ok() && IsWindowVisible(hwnd).as_bool()
    };
    let pixel_match_pct = pixel_match_ratio(buffer, &desktop);
    let nonblack_balance = if buffer_nonblack.max(desktop_nonblack) == 0 {
        0
    } else {
        (buffer_nonblack.min(desktop_nonblack) * 100) / buffer_nonblack.max(desktop_nonblack)
    };
    let pass = on_taskbar && buffer_nonblack > 20 && visible;

    let result = ProofResult {
        pass,
        on_taskbar,
        hwnd: hwnd.0 as isize,
        hwnd_rect: format!(
            "{},{},{},{}",
            hwnd_rect.left, hwnd_rect.top, hwnd_rect.right, hwnd_rect.bottom
        ),
        layered: format!("{layered_x},{layered_y},{width},{height}"),
        widget_region: format!("{cap_x},{cap_y},{cap_w},{cap_h}"),
        taskbar_band,
        fullscreen_rect: format!("{fs_x},{fs_y},{fs_w},{fs_h}"),
        buffer_nonblack,
        desktop_nonblack,
        buffer_hash,
        desktop_hash,
        buffer_matches_desktop: buffer_hash == desktop_hash,
        pixel_match_pct,
        buffer_len: buffer.len(),
        desktop_len: desktop.len(),
        visible,
        files: ProofFiles {
            buffer_bmp: buffer_path.to_string_lossy().into_owned(),
            desktop_bmp: desktop_path.to_string_lossy().into_owned(),
            fullscreen_bmp: fullscreen_path.to_string_lossy().into_owned(),
            manifest: manifest_path.to_string_lossy().into_owned(),
        },
    };

    match serde_json::to_string_pretty(&result) {
        Ok(json) => {
            if let Err(error) = fs::write(&manifest_path, json) {
                diagnose::log(format!("proof manifest write failed: {error}"));
            } else {
                diagnose::log(format!(
                    "proof written pass={pass} on_taskbar={on_taskbar} buffer_nb={buffer_nonblack} desktop_nb={desktop_nonblack}"
                ));
            }
        }
        Err(error) => diagnose::log(format!("proof json encode failed: {error}")),
    }
    let _ = fs::remove_file(flag);
}

fn virtual_screen_bounds() -> (i32, i32, i32, i32) {
    unsafe {
        let mut union = RECT {
            left: i32::MAX,
            top: i32::MAX,
            right: i32::MIN,
            bottom: i32::MIN,
        };
        unsafe extern "system" fn monitor_proc(
            _monitor: HMONITOR,
            _dc: HDC,
            rect: *mut RECT,
            lparam: LPARAM,
        ) -> windows::Win32::Foundation::BOOL {
            let union = &mut *(lparam.0 as *mut RECT);
            let r = *rect;
            union.left = union.left.min(r.left);
            union.top = union.top.min(r.top);
            union.right = union.right.max(r.right);
            union.bottom = union.bottom.max(r.bottom);
            windows::Win32::Foundation::BOOL(1)
        }
        let _ = EnumDisplayMonitors(
            HDC::default(),
            None,
            Some(monitor_proc),
            LPARAM(&mut union as *mut _ as isize),
        );
        if union.left < union.right && union.top < union.bottom {
            (
                union.left,
                union.top,
                union.right - union.left,
                union.bottom - union.top,
            )
        } else {
            let x = GetSystemMetrics(SYSTEM_METRICS_INDEX(76));
            let y = GetSystemMetrics(SYSTEM_METRICS_INDEX(77));
            let w = GetSystemMetrics(SYSTEM_METRICS_INDEX(78));
            let h = GetSystemMetrics(SYSTEM_METRICS_INDEX(79));
            (x, y, w, h)
        }
    }
}

struct MonitorCapture {
    rect: RECT,
    pixels: Vec<u32>,
}

fn capture_stitched_fullscreen(
    union_x: i32,
    union_y: i32,
    union_w: i32,
    union_h: i32,
) -> Result<Vec<u32>, String> {
    if union_w <= 0 || union_h <= 0 {
        return Err("invalid union dimensions".to_string());
    }
    let mut monitors: Vec<MonitorCapture> = Vec::new();
    unsafe {
        unsafe extern "system" fn monitor_proc(
            _monitor: HMONITOR,
            _dc: HDC,
            rect: *mut RECT,
            lparam: LPARAM,
        ) -> windows::Win32::Foundation::BOOL {
            let monitors = &mut *(lparam.0 as *mut Vec<MonitorCapture>);
            let r = *rect;
            let w = r.right - r.left;
            let h = r.bottom - r.top;
            if w <= 0 || h <= 0 {
                return windows::Win32::Foundation::BOOL(1);
            }
            match capture_desktop_bgra(r.left, r.top, w, h) {
                Ok(pixels) => monitors.push(MonitorCapture { rect: r, pixels }),
                Err(error) => diagnose::log(format!("monitor capture failed: {error}")),
            }
            windows::Win32::Foundation::BOOL(1)
        }
        let _ = EnumDisplayMonitors(
            HDC::default(),
            None,
            Some(monitor_proc),
            LPARAM(&mut monitors as *mut _ as isize),
        );
    }
    if monitors.is_empty() {
        return capture_desktop_bgra(union_x, union_y, union_w, union_h);
    }
    let w = union_w as usize;
    let h = union_h as usize;
    let mut out = vec![0u32; w * h];
    for mon in monitors {
        let mw = (mon.rect.right - mon.rect.left) as usize;
        let mh = (mon.rect.bottom - mon.rect.top) as usize;
        if mon.pixels.len() != mw * mh {
            continue;
        }
        for row in 0..mh {
            let dst_y = mon.rect.top + row as i32 - union_y;
            if dst_y < 0 || dst_y >= union_h {
                continue;
            }
            for col in 0..mw {
                let dst_x = mon.rect.left + col as i32 - union_x;
                if dst_x < 0 || dst_x >= union_w {
                    continue;
                }
                let src = row * mw + col;
                let dst = dst_y as usize * w + dst_x as usize;
                out[dst] = mon.pixels[src];
            }
        }
    }
    Ok(out)
}

fn count_nonblack_bgra(pixels: &[u32]) -> u32 {
    pixels
        .iter()
        .filter(|px| {
            let b = (*px & 0xFF) as u32;
            let g = ((*px >> 8) & 0xFF) as u32;
            let r = ((*px >> 16) & 0xFF) as u32;
            r > 15 || g > 15 || b > 15
        })
        .count() as u32
}

fn hash_pixels(pixels: &[u32]) -> u64 {
    let mut hash: u64 = 0xcbf29ce484222325;
    for px in pixels {
        hash ^= *px as u64;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

fn save_bmp32(path: &Path, width: i32, height: i32, pixels: &[u32]) -> Result<(), String> {
    if width <= 0 || height <= 0 {
        return Err("invalid dimensions".to_string());
    }
    let w = width as usize;
    let h = height as usize;
    let row_bytes = w * 4;
    let pixel_bytes = row_bytes * h;
    let file_size = 14 + 40 + pixel_bytes;
    let mut out = Vec::with_capacity(file_size);
    out.extend_from_slice(b"BM");
    out.extend_from_slice(&(file_size as u32).to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(&54u32.to_le_bytes());
    out.extend_from_slice(&40u32.to_le_bytes());
    out.extend_from_slice(&(width as i32).to_le_bytes());
    out.extend_from_slice(&(-(height as i32)).to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&32u16.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(&(pixel_bytes as u32).to_le_bytes());
    out.extend_from_slice(&0i32.to_le_bytes());
    out.extend_from_slice(&0i32.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    for row in 0..h {
        let start = row * w;
        let end = start + w;
        for px in &pixels[start..end] {
            let b = (px & 0xFF) as u8;
            let g = ((px >> 8) & 0xFF) as u8;
            let r = ((px >> 16) & 0xFF) as u8;
            out.push(b);
            out.push(g);
            out.push(r);
            out.push(0);
        }
    }
    fs::write(path, out).map_err(|e| e.to_string())
}

fn capture_desktop_bgra(x: i32, y: i32, width: i32, height: i32) -> Result<Vec<u32>, String> {
    if width <= 0 || height <= 0 {
        return Err("invalid dimensions".to_string());
    }
    unsafe {
        let screen_dc = GetDC(HWND::default());
        if screen_dc.is_invalid() {
            return Err("GetDC failed".to_string());
        }
        let mem_dc = CreateCompatibleDC(screen_dc);
        if mem_dc.is_invalid() {
            ReleaseDC(HWND::default(), screen_dc);
            return Err("CreateCompatibleDC failed".to_string());
        }
        let bmp = CreateCompatibleBitmap(screen_dc, width, height);
        if bmp.is_invalid() {
            let _ = DeleteDC(mem_dc);
            ReleaseDC(HWND::default(), screen_dc);
            return Err("CreateCompatibleBitmap failed".to_string());
        }
        let old = SelectObject(mem_dc, HGDIOBJ(bmp.0));
        let blt = BitBlt(mem_dc, 0, 0, width, height, screen_dc, x, y, SRCCOPY);
        if blt.is_err() {
            SelectObject(mem_dc, old);
            let _ = DeleteObject(bmp);
            let _ = DeleteDC(mem_dc);
            ReleaseDC(HWND::default(), screen_dc);
            return Err("BitBlt failed".to_string());
        }
        let mut bmi = BITMAPINFO {
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
        let count = (width * height) as usize;
        let mut pixels = vec![0u32; count];
        let ok = GetDIBits(
            mem_dc,
            bmp,
            0,
            height as u32,
            Some(pixels.as_mut_ptr() as *mut _),
            &mut bmi,
            DIB_RGB_COLORS,
        );
        SelectObject(mem_dc, old);
        let _ = DeleteObject(bmp);
        let _ = DeleteDC(mem_dc);
        ReleaseDC(HWND::default(), screen_dc);
        if ok == 0 {
            return Err("GetDIBits failed".to_string());
        }
        Ok(pixels)
    }
}

fn pixel_match_ratio(buffer: &[u32], desktop: &[u32]) -> u32 {
    if buffer.is_empty() || buffer.len() != desktop.len() {
        return 0;
    }
    let mut matched = 0u32;
    for (a, b) in buffer.iter().zip(desktop.iter()) {
        let ar = ((*a >> 16) & 0xFF) as i32;
        let ag = ((*a >> 8) & 0xFF) as i32;
        let ab = (*a & 0xFF) as i32;
        let br = ((*b >> 16) & 0xFF) as i32;
        let bg = ((*b >> 8) & 0xFF) as i32;
        let bb = (*b & 0xFF) as i32;
        if (ar - br).abs() <= 12 && (ag - bg).abs() <= 12 && (ab - bb).abs() <= 12 {
            matched += 1;
        }
    }
    ((matched as u64) * 100 / buffer.len() as u64) as u32
}
