//! Render theme surfaces to PNG files without starting the widget.
//!
//! `--render-theme` lets a theme pull request show what it draws, and the
//! snapshot tests in `tests` compare every theme in `src/themes` with the
//! reference images in `tests/snapshots/themes`. Both pin the clock, time zone,
//! appearance and asset folder through `FixedRenderEnvironment`, so a theme
//! and a usage fixture always give the same pixels.

#![allow(unsafe_code, reason = "attaches to the parent console through Win32")]

use std::path::{Path, PathBuf};

use crate::models::AppUsageData;
use crate::providers::{ProviderId, ProviderSet};
use crate::theme_engine::{
    bundled_theme, bundled_theme_ids, load_theme, render_theme_surface_with_runtime_at_scale,
    unknown_theme_fields, with_fixed_render_environment, FixedRenderEnvironment, RenderedTheme,
    SurfaceNest, ThemeDocument, ThemeRuntime,
};

/// Usage figures for Claude Code and Codex, timed against `DEFAULT_NOW_UNIX`.
pub const SAMPLE_USAGE: &str = include_str!("../tests/fixtures/theme-usage.json");
/// 2027-01-15 08:00:00 UTC.
pub const DEFAULT_NOW_UNIX: u64 = 1_800_000_000;

/// Host sizes used for layout expressions: a 1080p monitor with a 48 px taskbar.
const MONITOR_SIZE: (u32, u32) = (1920, 1080);
const TASKBAR_SIZE: (u32, u32) = (1920, 48);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Appearance {
    Dark,
    Light,
}

impl Appearance {
    pub const BOTH: [Self; 2] = [Self::Dark, Self::Light];

    pub fn name(self) -> &'static str {
        match self {
            Self::Dark => "dark",
            Self::Light => "light",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct RenderSettings {
    pub appearance: Appearance,
    pub now_unix: u64,
    pub utc_offset_minutes: i32,
    pub scale: f64,
    /// Folder for relative image paths; `None` uses the managed themes folder.
    pub asset_root: Option<PathBuf>,
}

impl Default for RenderSettings {
    fn default() -> Self {
        Self {
            appearance: Appearance::Dark,
            now_unix: DEFAULT_NOW_UNIX,
            utc_offset_minutes: 0,
            scale: 1.0,
            asset_root: None,
        }
    }
}

pub fn parse_usage(json: &str) -> Result<AppUsageData, String> {
    serde_json::from_str(json).map_err(|error| format!("invalid usage fixture: {error}"))
}

/// Render one surface the way the widget would, with the providers that have
/// data in `usage` enabled.
pub fn render_surface(
    theme: &ThemeDocument,
    surface_index: usize,
    usage: &AppUsageData,
    settings: &RenderSettings,
) -> RenderedTheme {
    let environment = FixedRenderEnvironment {
        now_unix: settings.now_unix,
        utc_offset_minutes: settings.utc_offset_minutes,
        dark: settings.appearance == Appearance::Dark,
        asset_root: settings.asset_root.clone(),
    };
    with_fixed_render_environment(environment, || {
        let runtime = runtime_for_surface(theme, surface_index, usage);
        render_theme_surface_with_runtime_at_scale(
            theme,
            surface_index,
            Some(usage),
            runtime,
            settings.scale,
        )
    })
}

fn runtime_for_surface(
    theme: &ThemeDocument,
    surface_index: usize,
    usage: &AppUsageData,
) -> ThemeRuntime {
    let providers = ProviderSet::from_enabled(
        ProviderId::ALL
            .into_iter()
            .filter(|provider| usage.get(*provider).is_some()),
    );
    let runtime = if providers.is_empty() {
        ThemeRuntime::default()
    } else {
        ThemeRuntime::from_providers(providers)
    };
    let Some(surface) = theme.surfaces.get(surface_index) else {
        return runtime;
    };
    let nest = surface
        .placement
        .nest
        .resolve(surface.placement.reference.region);
    let runtime = runtime.with_nest(nest);
    // Mirrors `window::host_geometry::runtime_with_geometry`.
    let (width, height) = match (nest, surface.placement.host_dimensions) {
        (SurfaceNest::Floating, Some(size)) => size,
        (SurfaceNest::Taskbar | SurfaceNest::TrayIcon, _) => TASKBAR_SIZE,
        _ => MONITOR_SIZE,
    };
    runtime.with_host_dimensions(width, height)
}

/// Convert premultiplied 0xAARRGGBB pixels to straight-alpha RGBA.
pub fn to_rgba_image(rendered: &RenderedTheme) -> image::RgbaImage {
    let mut bytes = Vec::with_capacity(rendered.pixels.len() * 4);
    for pixel in &rendered.pixels {
        let [blue, green, red, alpha] = pixel.to_le_bytes();
        let straight = |channel: u8| {
            if alpha == 0 {
                0
            } else {
                ((u32::from(channel) * 255 + u32::from(alpha) / 2) / u32::from(alpha)).min(255)
                    as u8
            }
        };
        bytes.extend([straight(red), straight(green), straight(blue), alpha]);
    }
    image::RgbaImage::from_raw(rendered.width, rendered.height, bytes)
        .expect("a rendered theme has width * height pixels")
}

/// `--render-theme` and `--theme-reference`: returns an exit code, or `None`
/// when neither flag is present.
pub fn handle_cli_mode(args: &[String]) -> Option<i32> {
    if args.iter().any(|arg| arg == "--theme-reference") {
        attach_parent_console();
        print!("{}", reference::theme_reference());
        return Some(0);
    }
    if !args.iter().any(|arg| arg == "--render-theme") {
        return None;
    }
    attach_parent_console();
    Some(match parse_args(args).and_then(|request| run(&request)) {
        Ok(written) => {
            for path in written {
                println!("{}", path.display());
            }
            0
        }
        Err(error) => {
            eprintln!("--render-theme: {error}");
            eprintln!("{USAGE}");
            2
        }
    })
}

const USAGE: &str = "\
usage: claude-code-usage-monitor --render-theme <theme.json | built-in id> --out <file.png | folder>
         [--fixture <usage.json>] [--surface <id | index | all>] [--appearance dark | light | both]
         [--now <unix seconds>] [--utc-offset <minutes>] [--scale <factor>]";

#[derive(Debug, PartialEq)]
struct RenderRequest {
    theme: String,
    out: PathBuf,
    fixture: Option<PathBuf>,
    surface: Option<String>,
    appearances: Vec<Appearance>,
    now_unix: u64,
    utc_offset_minutes: i32,
    scale: f64,
}

fn parse_args(args: &[String]) -> Result<RenderRequest, String> {
    let mut request = RenderRequest {
        theme: String::new(),
        out: PathBuf::new(),
        fixture: None,
        surface: None,
        appearances: Appearance::BOTH.to_vec(),
        now_unix: DEFAULT_NOW_UNIX,
        utc_offset_minutes: 0,
        scale: 1.0,
    };
    let mut theme = None;
    let mut out = None;
    let mut values = args.iter().skip(1);
    while let Some(flag) = values.next() {
        let mut value = || {
            values
                .next()
                .cloned()
                .ok_or_else(|| format!("{flag} needs a value"))
        };
        match flag.as_str() {
            "--render-theme" => theme = Some(value()?),
            "--out" => out = Some(PathBuf::from(value()?)),
            "--fixture" => request.fixture = Some(PathBuf::from(value()?)),
            "--surface" => {
                let surface = value()?;
                request.surface = (surface != "all").then_some(surface);
            }
            "--appearance" => {
                request.appearances = match value()?.as_str() {
                    "dark" => vec![Appearance::Dark],
                    "light" => vec![Appearance::Light],
                    "both" => Appearance::BOTH.to_vec(),
                    other => return Err(format!("unknown appearance '{other}'")),
                }
            }
            "--now" => {
                request.now_unix = value()?
                    .parse()
                    .map_err(|_| "--now takes Unix seconds".to_string())?;
            }
            "--utc-offset" => {
                request.utc_offset_minutes = value()?
                    .parse()
                    .ok()
                    .filter(|minutes: &i32| minutes.abs() <= 14 * 60)
                    .ok_or_else(|| "--utc-offset takes minutes from -840 to 840".to_string())?;
            }
            "--scale" => {
                request.scale = value()?
                    .parse()
                    .ok()
                    .filter(|scale: &f64| (0.5..=4.0).contains(scale))
                    .ok_or_else(|| "--scale takes a factor from 0.5 to 4".to_string())?;
            }
            "--diagnose" | "--diagnose-append" => {}
            other => return Err(format!("unknown argument '{other}'")),
        }
    }
    request.theme = theme.ok_or("--render-theme needs a theme file or built-in id")?;
    request.out = out.ok_or("--out is required")?;
    Ok(request)
}

fn run(request: &RenderRequest) -> Result<Vec<PathBuf>, String> {
    let (theme, asset_root) = load_requested_theme(&request.theme)?;
    let usage = match &request.fixture {
        Some(path) => parse_usage(
            &std::fs::read_to_string(path)
                .map_err(|error| format!("cannot read {}: {error}", path.display()))?,
        )?,
        None => parse_usage(SAMPLE_USAGE)?,
    };
    let surfaces = match &request.surface {
        None => (0..theme.surfaces.len()).collect::<Vec<_>>(),
        Some(wanted) => vec![theme
            .surfaces
            .iter()
            .position(|surface| surface.id == *wanted)
            .or_else(|| {
                wanted
                    .parse::<usize>()
                    .ok()
                    .filter(|index| *index < theme.surfaces.len())
            })
            .ok_or_else(|| {
                let ids = theme
                    .surfaces
                    .iter()
                    .map(|surface| surface.id.as_str())
                    .collect::<Vec<_>>();
                format!("no surface '{wanted}'; this theme has {}", ids.join(", "))
            })?],
    };
    let single_file = request
        .out
        .extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("png"));
    let count = surfaces.len() * request.appearances.len();
    if single_file && count != 1 {
        return Err(format!(
            "this renders {count} images; pass a folder to --out, or one --surface and --appearance"
        ));
    }
    if !single_file {
        std::fs::create_dir_all(&request.out)
            .map_err(|error| format!("cannot create {}: {error}", request.out.display()))?;
    }

    let mut written = Vec::new();
    for &surface_index in &surfaces {
        for &appearance in &request.appearances {
            let settings = RenderSettings {
                appearance,
                now_unix: request.now_unix,
                utc_offset_minutes: request.utc_offset_minutes,
                scale: request.scale,
                asset_root: asset_root.clone(),
            };
            let rendered = render_surface(&theme, surface_index, &usage, &settings);
            for warning in &rendered.warnings {
                eprintln!("warning: {warning}");
            }
            let path = if single_file {
                request.out.clone()
            } else {
                request.out.join(format!(
                    "{}-{}-{}.png",
                    theme.id,
                    theme.surfaces[surface_index].id,
                    appearance.name()
                ))
            };
            to_rgba_image(&rendered)
                .save(&path)
                .map_err(|error| format!("cannot write {}: {error}", path.display()))?;
            written.push(path);
        }
    }
    Ok(written)
}

/// A theme file renders with its images resolved next to it; a built-in id
/// uses the managed themes folder, where its assets are installed.
fn load_requested_theme(theme: &str) -> Result<(ThemeDocument, Option<PathBuf>), String> {
    let path = Path::new(theme);
    if path.is_file() {
        let document =
            load_theme(path).map_err(|error| format!("invalid theme {theme}: {error}"))?;
        // Serde ignores fields the format does not have; say which ones.
        if let Some(json) = std::fs::read_to_string(path)
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
        {
            for warning in unknown_theme_fields(&json) {
                eprintln!("warning: {warning}");
            }
        }
        let root = path
            .parent()
            .map(|parent| parent.to_path_buf())
            .filter(|parent| !parent.as_os_str().is_empty())
            .unwrap_or_else(|| PathBuf::from("."));
        return Ok((document, Some(root)));
    }
    match bundled_theme(theme) {
        Some(document) => Ok((document?, None)),
        None => Err(format!(
            "'{theme}' is not a file or a built-in theme id ({})",
            bundled_theme_ids().collect::<Vec<_>>().join(", ")
        )),
    }
}

/// The executable uses the Windows subsystem, so it has no console of its
/// own. Attach to the caller's console so messages reach the terminal.
fn attach_parent_console() {
    use windows::Win32::System::Console::{AttachConsole, ATTACH_PARENT_PROCESS};
    // SAFETY: AttachConsole takes no pointers. It fails harmlessly when the
    // parent has no console or this process already has one.
    let _ = unsafe { AttachConsole(ATTACH_PARENT_PROCESS) };
}

pub mod reference;

#[cfg(test)]
mod tests;
