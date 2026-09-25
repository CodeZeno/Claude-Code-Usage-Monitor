//! Native D3D11/WARP host. Runs in a fresh dashboard process after OpenGL fails.
//! Windows supplies the software rasterizer; the painter embeds precompiled shaders.

use super::{egui, style_native_titlebar, Page, StudioApp};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use std::time::{Duration, Instant};
use windows::core::{Interface, BOOL};
use windows::Win32::Foundation::{HMODULE, HWND};
use windows::Win32::Graphics::Direct3D::{D3D_DRIVER_TYPE_WARP, D3D_FEATURE_LEVEL_11_0};
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::Dxgi::{Common::*, *};
use winit::application::ApplicationHandler;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::window::{Window, WindowId};

struct Painter {
    device: ID3D11Device,
    context: ID3D11DeviceContext,
    swap_chain: IDXGISwapChain,
    target: Option<ID3D11RenderTargetView>,
    renderer: egui_directx11::Renderer,
    size: [u32; 2],
}

impl Painter {
    fn new(window: &Window) -> Result<Self, String> {
        let RawWindowHandle::Win32(handle) =
            window.window_handle().map_err(|e| e.to_string())?.as_raw()
        else {
            return Err("WARP requires a Windows window".into());
        };
        let size = window.inner_size();
        let desc = DXGI_SWAP_CHAIN_DESC {
            BufferDesc: DXGI_MODE_DESC {
                Width: size.width.max(1),
                Height: size.height.max(1),
                Format: DXGI_FORMAT_R8G8B8A8_UNORM,
                ..Default::default()
            },
            SampleDesc: DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0,
            },
            BufferUsage: DXGI_USAGE_RENDER_TARGET_OUTPUT,
            BufferCount: 2,
            OutputWindow: HWND(handle.hwnd.get() as *mut _),
            Windowed: BOOL(1),
            SwapEffect: DXGI_SWAP_EFFECT_FLIP_DISCARD,
            ..Default::default()
        };
        let (mut device, mut context, mut swap_chain) = (None, None, None);
        unsafe {
            D3D11CreateDeviceAndSwapChain(
                None,
                D3D_DRIVER_TYPE_WARP,
                HMODULE::default(),
                D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                Some(&[D3D_FEATURE_LEVEL_11_0]),
                D3D11_SDK_VERSION,
                Some(&desc),
                Some(&mut swap_chain),
                Some(&mut device),
                None,
                Some(&mut context),
            )
            .map_err(|e| format!("create D3D11 WARP device: {e}"))?;
        }
        let device = device.ok_or("WARP returned no device")?;
        let context = context.ok_or("WARP returned no context")?;
        let swap_chain = swap_chain.ok_or("WARP returned no swap chain")?;
        // Disable DXGI's automatic Alt+Enter transition; window state belongs to winit.
        unsafe {
            let factory: IDXGIFactory = swap_chain.GetParent().map_err(|e| e.to_string())?;
            factory
                .MakeWindowAssociation(desc.OutputWindow, DXGI_MWA_NO_ALT_ENTER)
                .map_err(|e| e.to_string())?;
            let adapter = device
                .cast::<IDXGIDevice>()
                .and_then(|d| d.GetAdapter())
                .map_err(|e| e.to_string())?;
            let info = adapter.GetDesc().map_err(|e| e.to_string())?;
            let name = String::from_utf16_lossy(&info.Description);
            crate::diagnose::log(format!(
                "dashboard renderer=D3D11 WARP adapter={}",
                name.trim_end_matches('\0')
            ));
        }
        let renderer = egui_directx11::Renderer::new(&device).map_err(|e| e.to_string())?;
        let mut painter = Self {
            device,
            context,
            swap_chain,
            renderer,
            target: None,
            size: [size.width.max(1), size.height.max(1)],
        };
        painter.create_target()?;
        Ok(painter)
    }

    fn create_target(&mut self) -> Result<(), String> {
        unsafe {
            let buffer: ID3D11Texture2D =
                self.swap_chain.GetBuffer(0).map_err(|e| e.to_string())?;
            self.device
                .CreateRenderTargetView(&buffer, None, Some(&mut self.target))
                .map_err(|e| e.to_string())?;
        }
        Ok(())
    }

    fn paint(
        &mut self,
        size: [u32; 2],
        context: &egui::Context,
        output: egui_directx11::RendererOutput,
    ) -> Result<(), String> {
        if size != self.size {
            unsafe {
                self.context.OMSetRenderTargets(None, None);
                self.target = None;
                self.swap_chain
                    .ResizeBuffers(
                        0,
                        size[0],
                        size[1],
                        DXGI_FORMAT_UNKNOWN,
                        DXGI_SWAP_CHAIN_FLAG(0),
                    )
                    .map_err(|e| e.to_string())?;
            }
            self.size = size;
            self.create_target()?;
        }
        let target = self.target.as_ref().ok_or("WARP render target missing")?;
        unsafe {
            self.context
                .ClearRenderTargetView(target, &[0.125, 0.125, 0.125, 1.0]);
        }
        self.renderer
            .render(&self.context, target, context, output)
            .map_err(|e| e.to_string())?;
        unsafe {
            self.swap_chain
                .Present(1, DXGI_PRESENT(0))
                .ok()
                .map_err(|e| format!("WARP present: {e}"))?;
        }
        Ok(())
    }
}

struct Desktop {
    // Drop graphics resources before the window.
    painter: Painter,
    app: StudioApp,
    state: egui_winit::State,
    window: Window,
    close_requested: bool,
}

struct Repaint {
    at: Instant,
    pass: u64,
}

struct Host {
    context: egui::Context,
    desktop: Option<Desktop>,
    width: f32,
    height: f32,
    icon: egui::IconData,
    owner: isize,
    page: Page,
    next_repaint: Instant,
    error: Option<String>,
    painted: bool,
}

impl Host {
    fn initialize(&mut self, event_loop: &ActiveEventLoop) -> Result<(), String> {
        let icon = winit::window::Icon::from_rgba(
            self.icon.rgba.clone(),
            self.icon.width,
            self.icon.height,
        )
        .map_err(|e| e.to_string())?;
        let window = event_loop
            .create_window(
                Window::default_attributes()
                    .with_title("Usage Monitor")
                    .with_visible(false)
                    .with_theme(Some(winit::window::Theme::Dark))
                    .with_inner_size(winit::dpi::LogicalSize::new(self.width, self.height))
                    .with_window_icon(Some(icon)),
            )
            .map_err(|e| e.to_string())?;
        if let Some(monitor) = window.current_monitor() {
            let size = window.outer_size();
            let screen = monitor.size();
            let pos = monitor.position();
            window.set_outer_position(winit::dpi::PhysicalPosition::new(
                pos.x + (screen.width.saturating_sub(size.width) / 2) as i32,
                pos.y + (screen.height.saturating_sub(size.height) / 2) as i32,
            ));
        }
        let painter = Painter::new(&window)?;
        let state = egui_winit::State::new(
            self.context.clone(),
            egui::ViewportId::ROOT,
            &window,
            Some(window.scale_factor() as f32),
            window.theme(),
            Some(16384),
        );
        self.context.set_embed_viewports(true);
        style_native_titlebar(&window);
        let app = StudioApp::new_with_context(&self.context, self.owner, self.page);
        self.desktop = Some(Desktop {
            painter,
            app,
            state,
            window,
            close_requested: false,
        });
        if let Some(desktop) = &self.desktop {
            desktop.window.set_visible(true);
            desktop.window.request_redraw();
        }
        Ok(())
    }

    fn draw(&mut self, event_loop: &ActiveEventLoop) -> Result<(), String> {
        let Some(desktop) = &mut self.desktop else {
            return Ok(());
        };
        let size = desktop.window.inner_size();
        if !desktop.close_requested
            && (size.width == 0 || size.height == 0 || desktop.window.is_minimized() == Some(true))
        {
            self.next_repaint = Instant::now() + Duration::from_millis(100);
            return Ok(());
        }
        let info = desktop
            .state
            .egui_input_mut()
            .viewports
            .entry(egui::ViewportId::ROOT)
            .or_default();
        egui_winit::update_viewport_info(info, &self.context, &desktop.window, !self.painted);
        if desktop.close_requested {
            info.events.push(egui::ViewportEvent::Close);
        }
        let input = desktop.state.take_egui_input(&desktop.window);
        let output = self.context.run_ui(input, |ui| desktop.app.draw(ui));
        let (render, platform, viewports) = egui_directx11::split_output(output);
        desktop
            .state
            .handle_platform_output(&desktop.window, platform);
        let mut close = desktop.close_requested;
        desktop.close_requested = false;
        let mut delay = Duration::from_millis(500);
        if let Some(root) = viewports.get(&egui::ViewportId::ROOT) {
            delay = root.repaint_delay;
            close = close_after_commands(close, &root.commands);
            let mut info = egui::ViewportInfo::default();
            egui_winit::process_viewport_commands(
                &self.context,
                &mut info,
                root.commands
                    .iter()
                    .filter(|cmd| {
                        !matches!(
                            cmd,
                            egui::ViewportCommand::Close | egui::ViewportCommand::CancelClose
                        )
                    })
                    .cloned(),
                &desktop.window,
                &mut Vec::new(),
            );
        }
        let paint_size = if size.width == 0 || size.height == 0 {
            desktop.painter.size
        } else {
            [size.width, size.height]
        };
        desktop.painter.paint(paint_size, &self.context, render)?;
        if !self.painted {
            crate::diagnose::log("dashboard WARP first frame presented");
            self.painted = true;
        }
        self.next_repaint = Instant::now() + delay.min(Duration::from_secs(1));
        if close {
            event_loop.exit();
        }
        Ok(())
    }

    fn fail(&mut self, event_loop: &ActiveEventLoop, error: String) {
        self.error = Some(error);
        event_loop.exit();
    }
}

fn close_after_commands(mut close: bool, commands: &[egui::ViewportCommand]) -> bool {
    for command in commands {
        match command {
            egui::ViewportCommand::Close => close = true,
            egui::ViewportCommand::CancelClose => close = false,
            _ => {}
        }
    }
    close
}

impl ApplicationHandler<Repaint> for Host {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.desktop.is_none() {
            if let Err(error) = self.initialize(event_loop) {
                self.fail(event_loop, error);
            }
        }
    }

    fn user_event(&mut self, _event_loop: &ActiveEventLoop, event: Repaint) {
        // Repaint callbacks can arrive after their frame was already rendered.
        let current = self.context.cumulative_pass_nr();
        if current == event.pass || current == event.pass.saturating_add(1) {
            self.next_repaint = self.next_repaint.min(event.at);
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, id: WindowId, event: WindowEvent) {
        let Some(desktop) = &mut self.desktop else {
            return;
        };
        if id != desktop.window.id() {
            return;
        }
        let response = desktop.state.on_window_event(&desktop.window, &event);
        // egui-winit marks RedrawRequested as needing paint too. That event is
        // already being serviced below; scheduling another would render forever.
        if response.repaint && !matches!(event, WindowEvent::RedrawRequested) {
            desktop.window.request_redraw();
        }
        match event {
            WindowEvent::CloseRequested => {
                desktop.close_requested = true;
                // Minimized windows may never receive RedrawRequested.
                if let Err(error) = self.draw(event_loop) {
                    self.fail(event_loop, error);
                }
            }
            WindowEvent::RedrawRequested => {
                if let Err(error) = self.draw(event_loop) {
                    self.fail(event_loop, error);
                }
            }
            WindowEvent::Destroyed => event_loop.exit(),
            _ => {}
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        if let Some(desktop) = &self.desktop {
            if self.next_repaint <= Instant::now() {
                desktop.window.request_redraw();
                // Hidden/minimized windows may not receive redraw events.
                self.next_repaint = Instant::now() + Duration::from_millis(100);
            }
            event_loop.set_control_flow(ControlFlow::WaitUntil(self.next_repaint));
        }
    }

    fn exiting(&mut self, _event_loop: &ActiveEventLoop) {
        if let Some(desktop) = self.desktop.take() {
            desktop.app.save_window_size();
        }
    }
}

pub(super) fn run(
    width: f32,
    height: f32,
    icon: egui::IconData,
    owner: isize,
    page: Page,
) -> Result<(), String> {
    let event_loop = EventLoop::<Repaint>::with_user_event()
        .build()
        .map_err(|e| e.to_string())?;
    let context = egui::Context::default();
    let proxy = event_loop.create_proxy();
    context.set_request_repaint_callback(move |info| {
        if let Some(at) = Instant::now().checked_add(info.delay) {
            let _ = proxy.send_event(Repaint {
                at,
                pass: info.current_cumulative_pass_nr,
            });
        }
    });
    let mut host = Host {
        context,
        desktop: None,
        width,
        height,
        icon,
        owner,
        page,
        next_repaint: Instant::now(),
        error: None,
        painted: false,
    };
    event_loop.run_app(&mut host).map_err(|e| e.to_string())?;
    host.error.map_or(Ok(()), Err)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn warp_renders_partial_textures_at_zoom_and_frees_after_paint() {
        use egui::epaint::{ClippedShape, ImageDelta, Mesh, Shape};
        let (mut device, mut immediate) = (None, None);
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
                Some(&mut immediate),
            )
            .unwrap();
        }
        let device = device.unwrap();
        let immediate = immediate.unwrap();
        let desc = D3D11_TEXTURE2D_DESC {
            Width: 16,
            Height: 16,
            MipLevels: 1,
            ArraySize: 1,
            Format: DXGI_FORMAT_R8G8B8A8_UNORM,
            SampleDesc: DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0,
            },
            Usage: D3D11_USAGE_DEFAULT,
            BindFlags: D3D11_BIND_RENDER_TARGET.0 as u32,
            ..Default::default()
        };
        let (mut texture, mut target, mut readback) = (None, None, None);
        unsafe {
            device
                .CreateTexture2D(&desc, None, Some(&mut texture))
                .unwrap();
            device
                .CreateRenderTargetView(texture.as_ref().unwrap(), None, Some(&mut target))
                .unwrap();
            device
                .CreateTexture2D(
                    &D3D11_TEXTURE2D_DESC {
                        Usage: D3D11_USAGE_STAGING,
                        BindFlags: 0,
                        CPUAccessFlags: D3D11_CPU_ACCESS_READ.0 as u32,
                        ..desc
                    },
                    None,
                    Some(&mut readback),
                )
                .unwrap();
        }
        let texture: ID3D11Texture2D = texture.unwrap();
        let target = target.unwrap();
        let readback: ID3D11Texture2D = readback.unwrap();
        let mut renderer = egui_directx11::Renderer::new(&device).unwrap();
        let context = egui::Context::default();
        context.set_zoom_factor(2.0);
        let mut warmup = context.run_ui(Default::default(), |_| {});
        warmup.textures_delta.clear();
        let id = egui::TextureId::Managed(123);
        let mut delta = egui::TexturesDelta::default();
        delta.set.entry(id).or_default().push(ImageDelta::full(
            egui::ColorImage::filled([3, 2], egui::Color32::RED),
            egui::TextureOptions::NEAREST,
        ));
        delta.set.entry(id).or_default().push(ImageDelta::partial(
            [2, 1],
            egui::ColorImage::filled([1, 1], egui::Color32::BLUE),
            egui::TextureOptions::NEAREST,
        ));
        delta.free.insert(id);
        let mut mesh = Mesh::with_texture(id);
        mesh.add_rect_with_uv(
            egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(3.0, 2.0)),
            egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0)),
            egui::Color32::WHITE,
        );
        let shapes = vec![ClippedShape {
            clip_rect: egui::Rect::EVERYTHING,
            shape: Shape::mesh(mesh),
        }];
        let read_pixel = |x: usize, y: usize| -> [u8; 4] {
            unsafe {
                immediate.CopyResource(&readback, &texture);
                let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
                immediate
                    .Map(&readback, 0, D3D11_MAP_READ, 0, Some(&mut mapped))
                    .unwrap();
                let pointer = mapped
                    .pData
                    .cast::<u8>()
                    .add(y * mapped.RowPitch as usize + x * 4);
                let pixel = [*pointer, *pointer.add(1), *pointer.add(2), *pointer.add(3)];
                immediate.Unmap(&readback, 0);
                pixel
            }
        };
        unsafe {
            immediate.ClearRenderTargetView(&target, &[0.0, 0.0, 0.0, 1.0]);
        }
        renderer
            .render(
                &immediate,
                &target,
                &context,
                egui_directx11::RendererOutput {
                    textures_delta: delta,
                    shapes: shapes.clone(),
                    pixels_per_point: 2.0,
                },
            )
            .unwrap();
        assert_eq!(read_pixel(1, 1), [255, 0, 0, 255]);
        assert_eq!(read_pixel(5, 3), [0, 0, 255, 255]);
        assert_eq!(
            read_pixel(8, 2),
            [0, 0, 0, 255],
            "zoom must only be applied once"
        );
        unsafe {
            immediate.ClearRenderTargetView(&target, &[0.0, 0.0, 0.0, 1.0]);
        }
        renderer
            .render(
                &immediate,
                &target,
                &context,
                egui_directx11::RendererOutput {
                    textures_delta: Default::default(),
                    shapes,
                    pixels_per_point: 2.0,
                },
            )
            .unwrap();
        assert_eq!(
            read_pixel(1, 1),
            [0, 0, 0, 255],
            "freed textures must not draw stale resources"
        );
    }

    #[test]
    fn unsaved_changes_can_cancel_close_then_close_after_confirmation() {
        assert!(!close_after_commands(
            true,
            &[egui::ViewportCommand::CancelClose]
        ));
        assert!(close_after_commands(false, &[egui::ViewportCommand::Close]));
        assert!(!close_after_commands(false, &[]));
        assert!(close_after_commands(true, &[]));
    }
}
