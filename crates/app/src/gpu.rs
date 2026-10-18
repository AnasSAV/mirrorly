//! D3D11 device on the adapter that drives the window's monitor, and a
//! flip-model swapchain tuned for latency: FLIP_DISCARD, frame-latency
//! waitable object, SetMaximumFrameLatency(1), tearing allowed if supported.

use anyhow::{Context, Result};
use windows::{
    Win32::{
        Foundation::{HANDLE, HMODULE, HWND},
        Graphics::{
            Direct3D::{D3D_DRIVER_TYPE_UNKNOWN, D3D_FEATURE_LEVEL, D3D_FEATURE_LEVEL_11_0, D3D_FEATURE_LEVEL_11_1},
            Direct3D11::{
                D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_CREATE_DEVICE_VIDEO_SUPPORT, D3D11_SDK_VERSION,
                D3D11CreateDevice, ID3D11Device, ID3D11DeviceContext, ID3D11Multithread,
            },
            Dxgi::{
                Common::{DXGI_ALPHA_MODE_IGNORE, DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_SAMPLE_DESC},
                CreateDXGIFactory2, DXGI_CREATE_FACTORY_FLAGS, DXGI_FEATURE_PRESENT_ALLOW_TEARING,
                DXGI_MWA_NO_ALT_ENTER, DXGI_SCALING_STRETCH, DXGI_SWAP_CHAIN_DESC1, DXGI_SWAP_CHAIN_FLAG,
                DXGI_SWAP_CHAIN_FLAG_ALLOW_TEARING, DXGI_SWAP_CHAIN_FLAG_FRAME_LATENCY_WAITABLE_OBJECT,
                DXGI_SWAP_EFFECT_FLIP_DISCARD, DXGI_USAGE_RENDER_TARGET_OUTPUT, IDXGIAdapter, IDXGIAdapter1,
                IDXGIFactory2, IDXGIFactory5, IDXGISwapChain1, IDXGISwapChain2,
            },
            Gdi::HMONITOR,
        },
    },
    core::{BOOL, Interface},
};

pub struct Gpu {
    pub device: ID3D11Device,
    pub context: ID3D11DeviceContext,
    pub adapter_name: String,
    factory: IDXGIFactory2,
    pub tearing: bool,
}

impl Gpu {
    /// Create the device on the adapter that owns `monitor` (so decode,
    /// render and scan-out stay on one GPU), else the default adapter.
    pub fn new(monitor: Option<HMONITOR>) -> Result<Self> {
        unsafe {
            let factory: IDXGIFactory2 = CreateDXGIFactory2(DXGI_CREATE_FACTORY_FLAGS(0))?;
            let (adapter, adapter_name) = pick_adapter(&factory, monitor)?;
            let levels: [D3D_FEATURE_LEVEL; 2] = [D3D_FEATURE_LEVEL_11_1, D3D_FEATURE_LEVEL_11_0];
            let mut device = None;
            let mut context = None;
            D3D11CreateDevice(
                &adapter,
                D3D_DRIVER_TYPE_UNKNOWN,
                HMODULE::default(),
                D3D11_CREATE_DEVICE_BGRA_SUPPORT | D3D11_CREATE_DEVICE_VIDEO_SUPPORT,
                Some(&levels),
                D3D11_SDK_VERSION,
                Some(&mut device),
                None,
                Some(&mut context),
            )
            .context("D3D11CreateDevice")?;
            let device = device.unwrap();
            let context = context.unwrap();
            // The decoder and the present thread share the immediate context
            // (under our own lock as well).
            if let Ok(mt) = context.cast::<ID3D11Multithread>() {
                let _ = mt.SetMultithreadProtected(true);
            }
            let tearing = factory.cast::<IDXGIFactory5>().is_ok_and(|f5| {
                let mut allow = BOOL(0);
                f5.CheckFeatureSupport(
                    DXGI_FEATURE_PRESENT_ALLOW_TEARING,
                    (&mut allow as *mut BOOL).cast(),
                    std::mem::size_of::<BOOL>() as u32,
                )
                .is_ok()
                    && allow.as_bool()
            });
            Ok(Self { device, context, adapter_name, factory, tearing })
        }
    }

    pub fn swapchain(&self, hwnd: HWND) -> Result<Swapchain> {
        unsafe {
            let mut flags = DXGI_SWAP_CHAIN_FLAG_FRAME_LATENCY_WAITABLE_OBJECT.0 as u32;
            if self.tearing {
                flags |= DXGI_SWAP_CHAIN_FLAG_ALLOW_TEARING.0 as u32;
            }
            let desc = DXGI_SWAP_CHAIN_DESC1 {
                Width: 0,
                Height: 0,
                Format: DXGI_FORMAT_B8G8R8A8_UNORM,
                SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
                BufferUsage: DXGI_USAGE_RENDER_TARGET_OUTPUT,
                BufferCount: 2,
                Scaling: DXGI_SCALING_STRETCH,
                SwapEffect: DXGI_SWAP_EFFECT_FLIP_DISCARD,
                AlphaMode: DXGI_ALPHA_MODE_IGNORE,
                Flags: flags,
                ..Default::default()
            };
            let sc1: IDXGISwapChain1 = self
                .factory
                .CreateSwapChainForHwnd(&self.device, hwnd, &desc, None, None)
                .context("CreateSwapChainForHwnd")?;
            let _ = self.factory.MakeWindowAssociation(hwnd, DXGI_MWA_NO_ALT_ENTER);
            let sc: IDXGISwapChain2 = sc1.cast()?;
            sc.SetMaximumFrameLatency(1)?;
            let waitable = sc.GetFrameLatencyWaitableObject();
            Ok(Swapchain { sc, waitable, flags: DXGI_SWAP_CHAIN_FLAG(flags as i32), tearing: self.tearing })
        }
    }
}

pub struct Swapchain {
    pub sc: IDXGISwapChain2,
    pub waitable: HANDLE,
    pub flags: DXGI_SWAP_CHAIN_FLAG,
    pub tearing: bool,
}

fn pick_adapter(factory: &IDXGIFactory2, monitor: Option<HMONITOR>) -> Result<(IDXGIAdapter, String)> {
    unsafe {
        let mut first: Option<(IDXGIAdapter1, String)> = None;
        let mut i = 0;
        while let Ok(a) = factory.EnumAdapters1(i) {
            i += 1;
            let desc = a.GetDesc1()?;
            let len = desc.Description.iter().position(|&c| c == 0).unwrap_or(desc.Description.len());
            let name = String::from_utf16_lossy(&desc.Description[..len]);
            if first.is_none() {
                first = Some((a.clone(), name.clone()));
            }
            let Some(mon) = monitor else { continue };
            let mut j = 0;
            while let Ok(out) = a.EnumOutputs(j) {
                j += 1;
                if out.GetDesc().is_ok_and(|d| d.Monitor == mon) {
                    return Ok((a.cast()?, name));
                }
            }
        }
        let (a, name) = first.context("no DXGI adapter")?;
        Ok((a.cast()?, name))
    }
}
