use std::cell::RefCell;
use std::collections::HashMap;
use std::ffi::c_void;

use windows::core::{Interface, BOOL};
use windows::Win32::Foundation::{HMODULE, HWND};
use windows::Win32::Graphics::Direct3D::{D3D_DRIVER_TYPE_HARDWARE, D3D_FEATURE_LEVEL};
use windows::Win32::Graphics::Direct3D11::{
    D3D11CreateDevice, ID3D11Device, ID3D11DeviceContext, ID3D11Texture2D,
    D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_SDK_VERSION,
};
use windows::Win32::Graphics::DirectComposition::{
    DCompositionCreateDevice, IDCompositionDevice, IDCompositionTarget, IDCompositionVisual,
};
use windows::Win32::Graphics::Dxgi::Common::{
    DXGI_ALPHA_MODE_PREMULTIPLIED, DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_SAMPLE_DESC,
};
use windows::Win32::Graphics::Dxgi::{
    IDXGIDevice, IDXGIFactory2, IDXGISwapChain1, DXGI_PRESENT, DXGI_SCALING_STRETCH,
    DXGI_SWAP_CHAIN_DESC1, DXGI_SWAP_EFFECT_FLIP_SEQUENTIAL, DXGI_USAGE_RENDER_TARGET_OUTPUT,
};

use crate::theme_engine::RenderedTheme;

thread_local! {
    // Desktop surfaces are created, rendered, and destroyed on the window
    // thread. Keeping their COM objects thread-local reflects that ownership
    // and avoids claiming the interfaces are Send/Sync.
    static PRESENTERS: RefCell<HashMap<isize, DesktopPresenter>> = RefCell::new(HashMap::new());
}

struct DesktopPresenter {
    _device: ID3D11Device,
    context: ID3D11DeviceContext,
    composition: IDCompositionDevice,
    _target: IDCompositionTarget,
    _visual: IDCompositionVisual,
    swap_chain: IDXGISwapChain1,
    width: u32,
    height: u32,
}

impl DesktopPresenter {
    unsafe fn create(hwnd: HWND, width: u32, height: u32) -> windows::core::Result<Self> {
        let mut device = None;
        let mut context = None;
        let mut feature_level = D3D_FEATURE_LEVEL::default();
        D3D11CreateDevice(
            None,
            D3D_DRIVER_TYPE_HARDWARE,
            HMODULE::default(),
            D3D11_CREATE_DEVICE_BGRA_SUPPORT,
            None,
            D3D11_SDK_VERSION,
            Some(&mut device),
            Some(&mut feature_level),
            Some(&mut context),
        )?;
        let device = device.expect("D3D11CreateDevice succeeded without a device");
        let context = context.expect("D3D11CreateDevice succeeded without a context");

        let dxgi_device: IDXGIDevice = device.cast()?;
        let adapter = dxgi_device.GetAdapter()?;
        let factory: IDXGIFactory2 = adapter.GetParent()?;
        let description = DXGI_SWAP_CHAIN_DESC1 {
            Width: width,
            Height: height,
            Format: DXGI_FORMAT_B8G8R8A8_UNORM,
            Stereo: BOOL(0),
            SampleDesc: DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0,
            },
            BufferUsage: DXGI_USAGE_RENDER_TARGET_OUTPUT,
            BufferCount: 2,
            Scaling: DXGI_SCALING_STRETCH,
            SwapEffect: DXGI_SWAP_EFFECT_FLIP_SEQUENTIAL,
            AlphaMode: DXGI_ALPHA_MODE_PREMULTIPLIED,
            Flags: 0,
        };
        let swap_chain = factory.CreateSwapChainForComposition(&device, &description, None)?;

        let composition: IDCompositionDevice = DCompositionCreateDevice(&dxgi_device)?;
        let target = composition.CreateTargetForHwnd(hwnd, true)?;
        let visual = composition.CreateVisual()?;
        visual.SetContent(&swap_chain)?;
        target.SetRoot(&visual)?;
        composition.Commit()?;

        Ok(Self {
            _device: device,
            context,
            composition,
            _target: target,
            _visual: visual,
            swap_chain,
            width,
            height,
        })
    }

    unsafe fn present(&self, rendered: &RenderedTheme) -> windows::core::Result<()> {
        let back_buffer: ID3D11Texture2D = self.swap_chain.GetBuffer(0)?;
        self.context.UpdateSubresource(
            &back_buffer,
            0,
            None,
            rendered.pixels.as_ptr().cast::<c_void>(),
            rendered.width.saturating_mul(4),
            0,
        );
        self.swap_chain.Present(1, DXGI_PRESENT(0)).ok()?;
        self.composition.Commit()
    }
}

pub fn present(hwnd: HWND, rendered: &RenderedTheme) -> Result<(), String> {
    if rendered.width == 0 || rendered.height == 0 {
        return Err("desktop surface has no pixels".into());
    }
    PRESENTERS.with(|presenters| {
        let mut presenters = presenters.borrow_mut();
        let key = hwnd.0 as isize;
        let size_changed = presenters.get(&key).is_some_and(|presenter| {
            presenter.width != rendered.width || presenter.height != rendered.height
        });
        if size_changed {
            presenters.remove(&key);
        }
        if let std::collections::hash_map::Entry::Vacant(entry) = presenters.entry(key) {
            let presenter =
                unsafe { DesktopPresenter::create(hwnd, rendered.width, rendered.height) }
                    .map_err(|error| format!("DirectComposition setup failed: {error}"))?;
            entry.insert(presenter);
        }
        unsafe { presenters[&key].present(rendered) }
            .map_err(|error| format!("DirectComposition present failed: {error}"))
    })
}

pub fn remove(hwnd: HWND) {
    PRESENTERS.with(|presenters| {
        presenters.borrow_mut().remove(&(hwnd.0 as isize));
    });
}

pub fn clear() {
    PRESENTERS.with(|presenters| presenters.borrow_mut().clear());
}
