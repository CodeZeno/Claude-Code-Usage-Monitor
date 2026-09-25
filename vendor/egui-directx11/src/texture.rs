// This file contains implementations inspired by or derived from the following
// sources:
// - https://github.com/ohchase/egui-directx/blob/master/egui-directx11/src/texture.rs
//
// Here I would express my gratitude for their contributions to the Rust
// community. Their work served as a valuable reference and inspiration for this
// project.
//
// Nekomaru, March 2024

use egui::{TextureFilter, TextureId, TextureOptions, TextureWrapMode, TexturesDelta};
use std::collections::HashMap;
use windows::{
    Win32::Graphics::{Direct3D11::*, Dxgi::Common::*},
    core::{Error, HRESULT, Result},
};

struct Texture {
    texture: Option<ID3D11Texture2D>,
    srv: ID3D11ShaderResourceView,
    sampler: ID3D11SamplerState,
    size: [usize; 2],
}

pub struct TexturePool {
    device: ID3D11Device,
    pool: HashMap<TextureId, Texture>,
    next_user_texture_id: u64,
}

impl TexturePool {
    pub fn new(device: &ID3D11Device) -> Self {
        Self {
            device: device.clone(),
            pool: HashMap::new(),
            next_user_texture_id: 0,
        }
    }
    pub fn get_srv(&self, id: TextureId) -> Option<(ID3D11ShaderResourceView, ID3D11SamplerState)> {
        self.pool
            .get(&id)
            .map(|t| (t.srv.clone(), t.sampler.clone()))
    }
    pub fn register_user_texture(&mut self, srv: ID3D11ShaderResourceView) -> TextureId {
        let id = TextureId::User(self.next_user_texture_id);
        self.next_user_texture_id += 1;
        let sampler = self.sampler(TextureOptions::LINEAR).expect("D3D11 sampler");
        self.pool.insert(
            id,
            Texture {
                texture: None,
                srv,
                sampler,
                size: [0, 0],
            },
        );
        id
    }
    pub fn unregister_user_texture(&mut self, id: TextureId) -> bool {
        if matches!(id, TextureId::User(_)) {
            self.pool.remove(&id).is_some()
        } else {
            false
        }
    }
    pub fn free(&mut self, ids: impl IntoIterator<Item = TextureId>) {
        for id in ids {
            self.pool.remove(&id);
        }
    }
    pub fn update(&mut self, ctx: &ID3D11DeviceContext, mut delta: TexturesDelta) -> Result<()> {
        for (id, updates) in delta.set.drain() {
            for delta in updates {
                let egui::ImageData::Color(image) = delta.image;
                let [width, height] = image.size;
                if width == 0 || height == 0 {
                    continue;
                }
                let sampler = self.sampler(delta.options)?;
                if let Some([x, y]) = delta.pos {
                    let old = self.pool.get_mut(&id).ok_or_else(invalid_texture)?;
                    let texture = old.texture.as_ref().ok_or_else(invalid_texture)?;
                    if x.checked_add(width).is_none_or(|end| end > old.size[0])
                        || y.checked_add(height).is_none_or(|end| end > old.size[1])
                    {
                        return Err(invalid_texture());
                    }
                    // UpdateSubresource accepts source row pitch explicitly. Never assume
                    // the driver's mapped texture rows are tightly packed (WARP pads them).
                    let region = D3D11_BOX {
                        left: x as u32,
                        top: y as u32,
                        front: 0,
                        right: (x + width) as u32,
                        bottom: (y + height) as u32,
                        back: 1,
                    };
                    unsafe {
                        ctx.UpdateSubresource(
                            texture,
                            0,
                            Some(&region),
                            image.pixels.as_ptr().cast(),
                            (width * 4) as u32,
                            0,
                        );
                    }
                    old.sampler = sampler;
                } else {
                    let desc = D3D11_TEXTURE2D_DESC {
                        Width: width as u32,
                        Height: height as u32,
                        MipLevels: 1,
                        ArraySize: 1,
                        Format: DXGI_FORMAT_R8G8B8A8_UNORM,
                        SampleDesc: DXGI_SAMPLE_DESC {
                            Count: 1,
                            Quality: 0,
                        },
                        Usage: D3D11_USAGE_DEFAULT,
                        BindFlags: D3D11_BIND_SHADER_RESOURCE.0 as u32,
                        ..Default::default()
                    };
                    let data = D3D11_SUBRESOURCE_DATA {
                        pSysMem: image.pixels.as_ptr().cast(),
                        SysMemPitch: (width * 4) as u32,
                        SysMemSlicePitch: 0,
                    };
                    let (mut texture, mut srv) = (None, None);
                    unsafe {
                        self.device
                            .CreateTexture2D(&desc, Some(&data), Some(&mut texture))?;
                        self.device.CreateShaderResourceView(
                            texture.as_ref().ok_or_else(invalid_texture)?,
                            None,
                            Some(&mut srv),
                        )?;
                    }
                    self.pool.insert(
                        id,
                        Texture {
                            texture,
                            srv: srv.ok_or_else(invalid_texture)?,
                            sampler,
                            size: image.size,
                        },
                    );
                }
            }
        }
        Ok(())
    }
    fn sampler(&self, options: TextureOptions) -> Result<ID3D11SamplerState> {
        let filter = match (options.minification, options.magnification) {
            (TextureFilter::Nearest, TextureFilter::Nearest) => D3D11_FILTER_MIN_MAG_MIP_POINT,
            (TextureFilter::Nearest, TextureFilter::Linear) => {
                D3D11_FILTER_MIN_POINT_MAG_LINEAR_MIP_POINT
            }
            (TextureFilter::Linear, TextureFilter::Nearest) => {
                D3D11_FILTER_MIN_LINEAR_MAG_MIP_POINT
            }
            (TextureFilter::Linear, TextureFilter::Linear) => D3D11_FILTER_MIN_MAG_LINEAR_MIP_POINT,
        };
        let wrap = match options.wrap_mode {
            TextureWrapMode::ClampToEdge => D3D11_TEXTURE_ADDRESS_CLAMP,
            TextureWrapMode::Repeat => D3D11_TEXTURE_ADDRESS_WRAP,
            TextureWrapMode::MirroredRepeat => D3D11_TEXTURE_ADDRESS_MIRROR,
        };
        let desc = D3D11_SAMPLER_DESC {
            Filter: filter,
            AddressU: wrap,
            AddressV: wrap,
            AddressW: wrap,
            ComparisonFunc: D3D11_COMPARISON_ALWAYS,
            MaxLOD: f32::MAX,
            ..Default::default()
        };
        let mut sampler = None;
        unsafe {
            self.device.CreateSamplerState(&desc, Some(&mut sampler))?;
        }
        sampler.ok_or_else(invalid_texture)
    }
}
fn invalid_texture() -> Error {
    Error::new(HRESULT(0x80070057u32 as i32), "Invalid egui texture update")
}
