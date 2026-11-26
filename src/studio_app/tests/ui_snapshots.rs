//! Dashboard pages drawn offscreen by the WARP renderer and compared with the
//! reference images in `tests/snapshots/ui`, so a UI change can be reviewed
//! as pictures. Every render is also written to `target/ui-snapshots`, which
//! CI uploads as an artifact.
#![allow(
    unsafe_code,
    reason = "creates a D3D11 WARP device and reads back its render target"
)]

use super::*;
use crate::snapshot_test::{
    check, manifest_dir, update_requested, Snapshots, PAGE_CHANGED_PIXEL_RATIO, UPDATE_SNAPSHOTS,
};
use crate::theme_engine::{with_fixed_render_environment, FixedRenderEnvironment};
use crate::theme_render::{parse_usage, DEFAULT_NOW_UNIX, SAMPLE_USAGE};
use windows::Win32::Foundation::HMODULE;
use windows::Win32::Graphics::Direct3D::{D3D_DRIVER_TYPE_WARP, D3D_FEATURE_LEVEL_11_0};
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT_R8G8B8A8_UNORM, DXGI_SAMPLE_DESC};

/// Logical size of the dashboard in the snapshots, drawn at 1 pixel per point.
const WIDTH: u32 = 1100;
const HEIGHT: u32 = 760;
/// Gives up waiting for the theme preview worker after this long.
const PREVIEW_TIMEOUT: Duration = Duration::from_secs(30);

/// A WARP device drawing into a texture instead of a window.
struct Offscreen {
    context: ID3D11DeviceContext,
    texture: ID3D11Texture2D,
    target: ID3D11RenderTargetView,
    staging: ID3D11Texture2D,
    renderer: egui_directx11::Renderer,
}

impl Offscreen {
    fn new() -> Self {
        let (mut device, mut context) = (None, None);
        // SAFETY: the feature-level slice and the out-params are live locals for the call, and
        // no window is involved.
        unsafe {
            D3D11CreateDevice(
                None,
                D3D_DRIVER_TYPE_WARP,
                HMODULE::default(),
                D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                Some(&[D3D_FEATURE_LEVEL_11_0]),
                D3D11_SDK_VERSION,
                Some(&mut device),
                None,
                Some(&mut context),
            )
            .expect("create a D3D11 WARP device");
        }
        let device: ID3D11Device = device.expect("WARP returned no device");
        let context = context.expect("WARP returned no context");
        let mut desc = D3D11_TEXTURE2D_DESC {
            Width: WIDTH,
            Height: HEIGHT,
            MipLevels: 1,
            ArraySize: 1,
            // The format the WARP dashboard's swap chain uses.
            Format: DXGI_FORMAT_R8G8B8A8_UNORM,
            SampleDesc: DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0,
            },
            Usage: D3D11_USAGE_DEFAULT,
            BindFlags: D3D11_BIND_RENDER_TARGET.0 as u32,
            ..Default::default()
        };
        let (mut texture, mut target, mut staging) = (None, None, None);
        // SAFETY: `desc` and the out-slots are live locals, the view is created from the
        // texture just created on the same device, and no initial data pointer is passed.
        unsafe {
            device
                .CreateTexture2D(&desc, None, Some(&mut texture))
                .expect("create the render target texture");
            device
                .CreateRenderTargetView(
                    texture.as_ref().expect("no render target texture"),
                    None,
                    Some(&mut target),
                )
                .expect("create the render target view");
            desc.Usage = D3D11_USAGE_STAGING;
            desc.BindFlags = 0;
            desc.CPUAccessFlags = D3D11_CPU_ACCESS_READ.0 as u32;
            device
                .CreateTexture2D(&desc, None, Some(&mut staging))
                .expect("create the readback texture");
        }
        let renderer = egui_directx11::Renderer::new(&device).expect("create the egui renderer");
        Self {
            context,
            texture: texture.expect("no render target texture"),
            target: target.expect("no render target view"),
            staging: staging.expect("no readback texture"),
            renderer,
        }
    }

    /// Draws one egui frame and returns its pixels.
    fn paint(
        &mut self,
        egui_context: &egui::Context,
        output: egui_directx11::RendererOutput,
    ) -> image::RgbaImage {
        // SAFETY: `target` is a live view on this device, and the color is a live array.
        unsafe {
            // The WARP dashboard's clear color.
            self.context
                .ClearRenderTargetView(&self.target, &[0.125, 0.125, 0.125, 1.0]);
        }
        self.renderer
            .render(&self.context, &self.target, egui_context, output)
            .expect("render the dashboard frame");
        let mut image = image::RgbaImage::new(WIDTH, HEIGHT);
        let row_bytes = WIDTH as usize * 4;
        // SAFETY: both textures are live, were created on this device with the same size and
        // format, and `staging` allows CPU reads. While it is mapped, `pData` points to
        // `HEIGHT` rows `RowPitch` bytes apart, each holding at least `row_bytes` bytes; the
        // slice is read before Unmap and `image` holds exactly `HEIGHT` rows of `row_bytes`.
        unsafe {
            self.context.CopyResource(&self.staging, &self.texture);
            let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
            self.context
                .Map(&self.staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped))
                .expect("map the readback texture");
            let pitch = mapped.RowPitch as usize;
            assert!(pitch >= row_bytes, "row pitch {pitch} is under {row_bytes}");
            let pixels = std::slice::from_raw_parts(
                mapped.pData.cast::<u8>(),
                pitch * (HEIGHT as usize - 1) + row_bytes,
            );
            for (row, out) in image.chunks_exact_mut(row_bytes).enumerate() {
                out.copy_from_slice(&pixels[row * pitch..][..row_bytes]);
            }
            self.context.Unmap(&self.staging, 0);
        }
        image
    }
}

/// A dashboard with the bundled themes, Classic v1 open and the sample usage,
/// reading nothing from this machine's settings, themes or usage cache.
fn snapshot_app(context: &egui::Context, themes: &Path) -> StudioApp {
    let path = themes.join("classic-usage-widget.json");
    let mut theme = theme_engine::load_theme(&path).expect("Classic v1 loads");
    theme.prepare_runtime();
    let mut app = app_with_surfaces(Vec::new());
    app.history_snapshot = theme.clone();
    app.theme = theme;
    app.theme_path = Some(path);
    app.available_themes = AvailableThemesCache::new(themes.to_path_buf());
    app.preview_renderer = PreviewRenderer::new(context.clone());
    app.usage = Some(parse_usage(SAMPLE_USAGE).expect("the sample usage parses"));
    app.usage_poll_ok = true;
    // Keep `refresh_usage_cache` from reading this machine's settings and usage.
    app.last_cache_read = Instant::now() + Duration::from_secs(24 * 60 * 60);
    app
}

/// Draws `app` until its layout settles and, on Theme Studio, the preview
/// arrives, then returns the last frame.
fn render_page(
    offscreen: &mut Offscreen,
    context: &egui::Context,
    app: &mut StudioApp,
) -> image::RgbaImage {
    let started = Instant::now();
    let mut settled_frames = 0;
    let mut frame = 0_u32;
    loop {
        let mut input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(WIDTH as f32, HEIGHT as f32),
            )),
            // A second per frame finishes every egui animation between frames.
            time: Some(f64::from(frame)),
            predicted_dt: 1.0,
            ..Default::default()
        };
        input
            .viewports
            .entry(egui::ViewportId::ROOT)
            .or_default()
            .native_pixels_per_point = Some(1.0);
        let output = context.run_ui(input, |ui| app.draw(ui));
        let (render, _, _) = egui_directx11::split_output(output);
        let image = offscreen.paint(context, render);
        frame += 1;

        let waiting_for_preview = app.page == Page::Studio && app.preview.is_none();
        if waiting_for_preview {
            assert!(
                started.elapsed() < PREVIEW_TIMEOUT,
                "the theme preview did not arrive"
            );
            std::thread::sleep(Duration::from_millis(20));
            continue;
        }
        settled_frames += 1;
        // egui sizes some panels from the previous frame; a few more frames settle them.
        if settled_frames >= 4 {
            return image;
        }
    }
}

/// Renders the Settings page and Theme Studio (with the canvas and with a
/// layer selected) and compares them with `tests/snapshots/ui/<page>.png`.
#[test]
fn dashboard_pages_match_reference_snapshots() {
    let root = manifest_dir();
    let references = root.join("tests/snapshots/ui");
    let actual_root = root.join("target/ui-snapshots");
    let snapshots = Snapshots {
        references: &references,
        actual_root: &actual_root,
        update: update_requested(),
        max_changed_ratio: PAGE_CHANGED_PIXEL_RATIO,
    };
    // Copies of the bundled themes, so the theme list does not show this
    // machine's themes.
    let themes = tempfile::tempdir().unwrap();
    for entry in std::fs::read_dir(root.join("src/themes")).unwrap() {
        let path = entry.unwrap().path();
        if path
            .extension()
            .is_some_and(|extension| extension == "json")
        {
            std::fs::copy(&path, themes.path().join(path.file_name().unwrap())).unwrap();
        }
    }
    let environment = FixedRenderEnvironment {
        now_unix: DEFAULT_NOW_UNIX,
        utc_offset_minutes: 0,
        dark: true,
        asset_root: Some(root.join("src/themes")),
    };

    let pages: [(&str, Page, Selection); 3] = [
        ("settings", Page::Settings, Selection::Surface(0)),
        ("theme-studio", Page::Studio, Selection::Surface(0)),
        ("theme-studio-layer", Page::Studio, Selection::Object(0, 0)),
    ];
    let mut offscreen = Offscreen::new();
    let mut failures = Vec::new();
    with_fixed_render_environment(environment, || {
        for (name, page, selection) in pages {
            // A fresh context per page, so one page's layout memory does not
            // leak into the next.
            let context = egui::Context::default();
            configure_style(&context, LanguageId::English);
            let mut app = snapshot_app(&context, themes.path());
            app.page = page;
            app.selection = selection;
            let image = render_page(&mut offscreen, &context, &mut app);
            let relative = PathBuf::from(format!("{name}.png"));
            if let Some(problem) = check(&image, &relative, &snapshots) {
                failures.push(format!("{name}: {problem}"));
            }
        }
    });

    assert!(
        failures.is_empty(),
        "dashboard snapshots differ:\n  {}\n\nRenders are in {}. If the change is intended, \
         run `$env:{UPDATE_SNAPSHOTS}=1; cargo test --locked ui_snapshots` (PowerShell) or \
         `{UPDATE_SNAPSHOTS}=1 cargo test --locked ui_snapshots` (bash), check the new images \
         in tests/snapshots/ui, and commit them.",
        failures.join("\n  "),
        actual_root.display()
    );
}
