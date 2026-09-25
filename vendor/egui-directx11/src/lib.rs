#![warn(missing_docs)]

//! `egui-directx11`: a Direct3D11 renderer for [`egui`](https://crates.io/crates/egui).
//!
//! This crate aims to provide a *minimal* set of features and APIs to render
//! outputs from `egui` using Direct3D11. We assume you to be familiar with
//! developing graphics applications using Direct3D11, and if not, this crate is
//! not likely useful for you. Besides, this crate cares only about rendering
//! outputs from `egui`, so it is all *your* responsibility to handle things
//! like setting up the window and event loop, creating the device and swap
//! chain, etc.
//!
//! This crate is built upon the *official* Rust bindings of Direct3D11 and DXGI
//! APIs from the [`windows`](https://crates.io/crates/windows) crate [maintained by
//! Microsoft](https://github.com/microsoft/windows-rs). Using this crate with
//! other Direct3D11 bindings is not recommended and may result in unexpected
//! behavior.
//!
//! To get started, you can check the [`Renderer`] struct provided by this
//! crate. You can also take a look at the [example](https://github.com/Nekomaru-PKU/egui-directx11/blob/main/examples/main.rs), which demonstrates all you need to do to set up a minimal application
//! with Direct3D11 and `egui`. This example uses `winit` for window management
//! and event handling, while native Win32 APIs should also work well.

mod texture;
use texture::TexturePool;

use std::mem;

const fn zeroed<T>() -> T {
    unsafe { mem::zeroed() }
}

use egui::{
    ClippedPrimitive, Pos2,
    epaint::{ClippedShape, Primitive, Vertex, textures::TexturesDelta},
};

use windows::Win32::Foundation::RECT;
use windows::Win32::Graphics::{Direct3D::*, Direct3D11::*, Dxgi::Common::*};
use windows::core::BOOL;
use windows::core::{Interface, Result};

/// The core of this crate. You can set up a renderer via [`Renderer::new`]
/// and render the output from `egui` with [`Renderer::render`].
pub struct Renderer {
    device: ID3D11Device,
    input_layout: ID3D11InputLayout,
    vertex_shader: ID3D11VertexShader,
    pixel_shader: ID3D11PixelShader,
    rasterizer_state: ID3D11RasterizerState,
    sampler_state: ID3D11SamplerState,
    blend_state: ID3D11BlendState,

    texture_pool: TexturePool,
}

/// Part of [`egui::FullOutput`] that is consumed by [`Renderer::render`].
///
/// Call to [`egui::Context::run`] or [`egui::Context::end_frame`] yields a
/// [`egui::FullOutput`]. The platform integration (for example `egui_winit`)
/// consumes [`egui::FullOutput::platform_output`] and
/// [`egui::FullOutput::viewport_output`], and the renderer consumes the rest.
///
/// To conveniently split a [`egui::FullOutput`] into a [`RendererOutput`] and
/// outputs for the platform integration, use [`split_output`].
#[allow(missing_docs)]
pub struct RendererOutput {
    pub textures_delta: TexturesDelta,
    pub shapes: Vec<ClippedShape>,
    pub pixels_per_point: f32,
}

/// Convenience method to split a [`egui::FullOutput`] into the
/// [`RendererOutput`] part and other parts for platform integration.
///
/// The returned tuple should be destructured as:
/// ```ignore
/// let (renderer_output, platform_output, viewport_output) =
///     egui_directx11::split_output(full_output);
/// ```
pub fn split_output(
    full_output: egui::FullOutput,
) -> (
    RendererOutput,
    egui::PlatformOutput,
    egui::OrderedViewportIdMap<egui::ViewportOutput>,
) {
    (
        RendererOutput {
            textures_delta: full_output.textures_delta,
            shapes: full_output.shapes,
            pixels_per_point: full_output.pixels_per_point,
        },
        full_output.platform_output,
        full_output.viewport_output,
    )
}

#[repr(C)]
struct VertexData {
    pos: Pos2,
    uv: Pos2,
    color: [f32; 4],
}

struct MeshData {
    vtx: Vec<VertexData>,
    idx: Vec<u32>,
    tex: egui::TextureId,
    clip_rect: egui::Rect,
}

impl Renderer {
    /// Create a [`Renderer`] using the provided Direct3D11 device. The
    /// [`Renderer`] holds various Direct3D11 resources and states derived
    /// from the device.
    ///
    /// If any Direct3D resource creation fails, this function will return an
    /// error. You can create the Direct3D11 device with debug layer enabled
    /// to find out details on the error.
    pub fn new(device: &ID3D11Device) -> Result<Self> {
        let mut input_layout = None;
        let mut vertex_shader = None;
        let mut pixel_shader = None;
        let mut rasterizer_state = None;
        let mut sampler_state = None;
        let mut blend_state = None;
        unsafe {
            device.CreateInputLayout(
                &Self::INPUT_ELEMENTS_DESC,
                Self::VS_BLOB,
                Some(&mut input_layout),
            )?;
            device.CreateVertexShader(Self::VS_BLOB, None, Some(&mut vertex_shader))?;
            device.CreatePixelShader(Self::PS_BLOB, None, Some(&mut pixel_shader))?;
            device.CreateRasterizerState(&Self::RASTERIZER_DESC, Some(&mut rasterizer_state))?;
            device.CreateSamplerState(&Self::SAMPLER_DESC, Some(&mut sampler_state))?;
            device.CreateBlendState(&Self::BLEND_DESC, Some(&mut blend_state))?;
        };
        Ok(Self {
            device: device.clone(),
            input_layout: input_layout.unwrap(),
            vertex_shader: vertex_shader.unwrap(),
            pixel_shader: pixel_shader.unwrap(),
            rasterizer_state: rasterizer_state.unwrap(),
            sampler_state: sampler_state.unwrap(),
            blend_state: blend_state.unwrap(),
            texture_pool: TexturePool::new(device),
        })
    }

    /// Register a user-provided `ID3D11ShaderResourceView` and get a [`egui::TextureId`] for it.
    ///
    /// This allows you to use your own DirectX11 textures within egui. The returned
    /// [`egui::TextureId`] can be used with [`egui::Image`], [`egui::ImageButton`], or
    /// any other egui widget that accepts a texture ID.
    ///
    /// The texture will remain registered until you call [`Renderer::unregister_user_texture`]
    /// or the [`Renderer`] is dropped.
    ///
    /// # Example
    ///
    /// ```ignore
    /// // Assuming you have a ID3D11ShaderResourceView
    /// let texture_id = renderer.register_user_texture(my_srv);
    ///
    /// // Use it in egui
    /// ui.image(egui::ImageSource::Texture(egui::load::SizedTexture::new(
    ///     texture_id,
    ///     egui::vec2(256.0, 256.0),
    /// )));
    /// ```
    pub fn register_user_texture(&mut self, srv: ID3D11ShaderResourceView) -> egui::TextureId {
        self.texture_pool.register_user_texture(srv)
    }

    /// Unregister a user texture by its [`egui::TextureId`].
    ///
    /// Returns `true` if the texture was found and removed, `false` otherwise.
    /// Note that this only works for user-registered textures, not textures
    /// managed by egui itself.
    pub fn unregister_user_texture(&mut self, tid: egui::TextureId) -> bool {
        self.texture_pool.unregister_user_texture(tid)
    }

    /// Render the output of `egui` to the provided `render_target`.
    ///
    /// As `egui` requires color blending in gamma space, **the provided
    /// `render_target` MUST be in the gamma color space and viewed as
    /// non-sRGB-aware** (i.e. do NOT use `_SRGB` format in the texture and
    /// the view).
    ///
    /// If you have to render to a render target in linear color space or
    /// one that is sRGB-aware, you must create an intermediate render target
    /// in gamma color space and perform a blit operation afterwards.
    ///
    /// The `scale_factor` should be the scale factor of your window and not
    /// confused with [`egui::Context::zoom_factor`]. If you are using `winit`,
    /// the `scale_factor` can be aquired using `Window::scale_factor`.
    ///
    /// ## Error Handling
    ///
    /// If any Direct3D resource creation fails, this function will return an
    /// error. In this case you may have a incomplete or incorrect rendering
    /// result. You can create the Direct3D11 device with debug layer
    /// enabled to find out details on the error.
    /// If the device has been lost, you should drop the [`Renderer`] and create
    /// a new one.
    ///
    /// ## Pipeline State Management
    ///
    /// This function sets up its own Direct3D11 pipeline state for rendering on
    /// the provided device context. It assumes that the hull shader, domain
    /// shader and geometry shader stages are not active on the provided device
    /// context without any further checks. It is all *your* responsibility to
    /// backup the current pipeline state and restore it afterwards if your
    /// rendering pipeline depends on it.
    ///
    /// Particularly, it overrides:
    /// + The input layout, vertex buffer, index buffer and primitive topology
    ///   in the input assembly stage;
    /// + The current shader in the vertex shader stage;
    /// + The viewport and rasterizer state in the rasterizer stage;
    /// + The current shader, shader resource slot 0 and sampler slot 0 in the
    ///   pixel shader stage;
    /// + The render target(s) and blend state in the output merger stage;
    pub fn render(
        &mut self,
        device_context: &ID3D11DeviceContext,
        render_target: &ID3D11RenderTargetView,
        egui_ctx: &egui::Context,
        egui_output: RendererOutput,
    ) -> Result<()> {
        let mut delta = egui_output.textures_delta;
        let free = std::mem::take(&mut delta.free);
        self.texture_pool.update(device_context, delta)?;

        if egui_output.shapes.is_empty() {
            self.texture_pool.free(free);
            return Ok(());
        }

        let frame_size = Self::get_render_target_size(render_target)?;
        let frame_size_scaled = (
            frame_size.0 as f32 / egui_output.pixels_per_point,
            frame_size.1 as f32 / egui_output.pixels_per_point,
        );
        // pixels_per_point already includes the egui zoom factor.

        self.setup(device_context, render_target, frame_size);
        let meshes = egui_ctx
            .tessellate(egui_output.shapes, egui_output.pixels_per_point)
            .into_iter()
            .filter_map(
                |ClippedPrimitive {
                     primitive,
                     clip_rect,
                 }| match primitive {
                    Primitive::Mesh(mesh) => Some((mesh, clip_rect)),
                    Primitive::Callback(..) => {
                        log::warn!("paint callbacks are not yet supported.");
                        None
                    }
                },
            )
            .filter_map(|(mesh, clip_rect)| {
                if mesh.indices.is_empty() {
                    return None;
                }
                if mesh.indices.len() % 3 != 0 {
                    log::warn!(concat!(
                        "egui wants to draw a incomplete triangle. ",
                        "this request will be ignored."
                    ));
                    return None;
                }
                Some(MeshData {
                    vtx: mesh
                        .vertices
                        .into_iter()
                        .map(|Vertex { pos, uv, color }| VertexData {
                            pos: Pos2::new(
                                pos.x / frame_size_scaled.0 * 2.0 - 1.0,
                                1.0 - pos.y / frame_size_scaled.1 * 2.0,
                            ),
                            uv,
                            color: [
                                color[0] as f32 / 255.0,
                                color[1] as f32 / 255.0,
                                color[2] as f32 / 255.0,
                                color[3] as f32 / 255.0,
                            ],
                        })
                        .collect(),
                    idx: mesh.indices,
                    tex: mesh.texture_id,
                    clip_rect: (clip_rect * egui_output.pixels_per_point).intersect(
                        egui::Rect::from_min_size(
                            Pos2::ZERO,
                            egui::vec2(frame_size.0 as f32, frame_size.1 as f32),
                        ),
                    ),
                })
            });
        for mesh in meshes {
            Self::draw_mesh(&self.device, device_context, &self.texture_pool, mesh)?;
        }
        self.texture_pool.free(free);
        Ok(())
    }

    fn setup(
        &mut self,
        ctx: &ID3D11DeviceContext,
        render_target: &ID3D11RenderTargetView,
        frame_size: (u32, u32),
    ) {
        unsafe {
            ctx.IASetPrimitiveTopology(D3D11_PRIMITIVE_TOPOLOGY_TRIANGLELIST);
            ctx.IASetInputLayout(&self.input_layout);
            ctx.VSSetShader(&self.vertex_shader, None);
            ctx.PSSetShader(&self.pixel_shader, None);
            ctx.RSSetState(&self.rasterizer_state);
            ctx.RSSetViewports(Some(&[D3D11_VIEWPORT {
                TopLeftX: 0.,
                TopLeftY: 0.,
                Width: frame_size.0 as _,
                Height: frame_size.1 as _,
                MinDepth: 0.,
                MaxDepth: 1.,
            }]));
            ctx.PSSetSamplers(0, Some(&[Some(self.sampler_state.clone())]));
            ctx.OMSetRenderTargets(Some(&[Some(render_target.clone())]), None);
            ctx.OMSetBlendState(&self.blend_state, Some(&[0.; 4]), u32::MAX);
        }
    }

    fn draw_mesh(
        device: &ID3D11Device,
        device_context: &ID3D11DeviceContext,
        texture_pool: &TexturePool,
        mesh: MeshData,
    ) -> Result<()> {
        let vb = Self::create_index_buffer(device, &mesh.idx)?;
        let ib = Self::create_vertex_buffer(device, &mesh.vtx)?;
        unsafe {
            device_context.IASetVertexBuffers(
                0,
                1,
                Some(&Some(ib)),
                Some(&(mem::size_of::<VertexData>() as _)),
                Some(&0),
            );
            device_context.IASetIndexBuffer(&vb, DXGI_FORMAT_R32_UINT, 0);
            device_context.RSSetScissorRects(Some(&[RECT {
                left: mesh.clip_rect.left() as _,
                top: mesh.clip_rect.top() as _,
                right: mesh.clip_rect.right() as _,
                bottom: mesh.clip_rect.bottom() as _,
            }]));
        }
        if let Some((srv, sampler)) = texture_pool.get_srv(mesh.tex) {
            unsafe {
                device_context.PSSetSamplers(0, Some(&[Some(sampler)]));
            }
            unsafe { device_context.PSSetShaderResources(0, Some(&[Some(srv)])) };
        } else {
            log::warn!(
                concat!(
                    "egui wants to sample a non-existing texture {:?}.",
                    "this request will be ignored."
                ),
                mesh.tex
            );
            return Ok(());
        };
        unsafe { device_context.DrawIndexed(mesh.idx.len() as _, 0, 0) };
        Ok(())
    }
}

impl Renderer {
    const VS_BLOB: &'static [u8] = include_bytes!("../shaders/vs_egui.bin");
    const PS_BLOB: &'static [u8] = include_bytes!("../shaders/ps_egui.bin");

    const INPUT_ELEMENTS_DESC: [D3D11_INPUT_ELEMENT_DESC; 3] = [
        D3D11_INPUT_ELEMENT_DESC {
            SemanticName: windows::core::s!("POSITION"),
            SemanticIndex: 0,
            Format: DXGI_FORMAT_R32G32_FLOAT,
            InputSlot: 0,
            AlignedByteOffset: 0,
            InputSlotClass: D3D11_INPUT_PER_VERTEX_DATA,
            InstanceDataStepRate: 0,
        },
        D3D11_INPUT_ELEMENT_DESC {
            SemanticName: windows::core::s!("TEXCOORD"),
            SemanticIndex: 0,
            Format: DXGI_FORMAT_R32G32_FLOAT,
            InputSlot: 0,
            AlignedByteOffset: D3D11_APPEND_ALIGNED_ELEMENT,
            InputSlotClass: D3D11_INPUT_PER_VERTEX_DATA,
            InstanceDataStepRate: 0,
        },
        D3D11_INPUT_ELEMENT_DESC {
            SemanticName: windows::core::s!("COLOR"),
            SemanticIndex: 0,
            Format: DXGI_FORMAT_R32G32B32A32_FLOAT,
            InputSlot: 0,
            AlignedByteOffset: D3D11_APPEND_ALIGNED_ELEMENT,
            InputSlotClass: D3D11_INPUT_PER_VERTEX_DATA,
            InstanceDataStepRate: 0,
        },
    ];

    const RASTERIZER_DESC: D3D11_RASTERIZER_DESC = D3D11_RASTERIZER_DESC {
        FillMode: D3D11_FILL_SOLID,
        CullMode: D3D11_CULL_NONE,
        FrontCounterClockwise: BOOL(0),
        DepthBias: 0,
        DepthBiasClamp: 0.,
        SlopeScaledDepthBias: 0.,
        DepthClipEnable: BOOL(0),
        ScissorEnable: BOOL(1),
        MultisampleEnable: BOOL(0),
        AntialiasedLineEnable: BOOL(0),
    };

    const SAMPLER_DESC: D3D11_SAMPLER_DESC = D3D11_SAMPLER_DESC {
        Filter: D3D11_FILTER_MIN_MAG_MIP_LINEAR,
        AddressU: D3D11_TEXTURE_ADDRESS_BORDER,
        AddressV: D3D11_TEXTURE_ADDRESS_BORDER,
        AddressW: D3D11_TEXTURE_ADDRESS_BORDER,
        ComparisonFunc: D3D11_COMPARISON_ALWAYS,
        BorderColor: [1., 1., 1., 1.],
        ..zeroed()
    };

    const BLEND_DESC: D3D11_BLEND_DESC = D3D11_BLEND_DESC {
        RenderTarget: [
            D3D11_RENDER_TARGET_BLEND_DESC {
                BlendEnable: BOOL(1),
                SrcBlend: D3D11_BLEND_ONE,
                DestBlend: D3D11_BLEND_INV_SRC_ALPHA,
                BlendOp: D3D11_BLEND_OP_ADD,
                SrcBlendAlpha: D3D11_BLEND_INV_DEST_ALPHA,
                DestBlendAlpha: D3D11_BLEND_ONE,
                BlendOpAlpha: D3D11_BLEND_OP_ADD,
                RenderTargetWriteMask: D3D11_COLOR_WRITE_ENABLE_ALL.0 as _,
            },
            zeroed(),
            zeroed(),
            zeroed(),
            zeroed(),
            zeroed(),
            zeroed(),
            zeroed(),
        ],
        ..zeroed()
    };
}

impl Renderer {
    fn create_vertex_buffer(device: &ID3D11Device, data: &[VertexData]) -> Result<ID3D11Buffer> {
        let mut vertex_buffer = None;
        unsafe {
            device.CreateBuffer(
                &D3D11_BUFFER_DESC {
                    ByteWidth: mem::size_of_val(data) as _,
                    Usage: D3D11_USAGE_IMMUTABLE,
                    BindFlags: D3D11_BIND_VERTEX_BUFFER.0 as _,
                    ..D3D11_BUFFER_DESC::default()
                },
                Some(&D3D11_SUBRESOURCE_DATA {
                    pSysMem: data.as_ptr() as _,
                    ..D3D11_SUBRESOURCE_DATA::default()
                }),
                Some(&mut vertex_buffer),
            )
        }?;
        Ok(vertex_buffer.unwrap())
    }

    fn create_index_buffer(device: &ID3D11Device, data: &[u32]) -> Result<ID3D11Buffer> {
        let mut index_buffer = None;
        unsafe {
            device.CreateBuffer(
                &D3D11_BUFFER_DESC {
                    ByteWidth: mem::size_of_val(data) as _,
                    Usage: D3D11_USAGE_IMMUTABLE,
                    BindFlags: D3D11_BIND_INDEX_BUFFER.0 as _,
                    ..D3D11_BUFFER_DESC::default()
                },
                Some(&D3D11_SUBRESOURCE_DATA {
                    pSysMem: data.as_ptr() as _,
                    ..D3D11_SUBRESOURCE_DATA::default()
                }),
                Some(&mut index_buffer),
            )
        }?;
        Ok(index_buffer.unwrap())
    }

    fn get_render_target_size(rtv: &ID3D11RenderTargetView) -> Result<(u32, u32)> {
        let tex = unsafe { rtv.GetResource() }?.cast::<ID3D11Texture2D>()?;
        let mut desc = zeroed();
        unsafe { tex.GetDesc(&mut desc) };
        Ok((desc.Width, desc.Height))
    }
}
