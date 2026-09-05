//! Extração do ícone do executável (SHGetFileInfoW → HICON → RGBA). No macOS/Linux: None.

#[cfg(windows)]
use std::ffi::c_void;
#[cfg(windows)]
use windows::core::PCWSTR;
#[cfg(windows)]
use windows::Win32::Graphics::Gdi::{
    CreateCompatibleDC, DeleteDC, DeleteObject, GetDIBits, GetObjectW, BITMAP, BITMAPINFO, BITMAPINFOHEADER,
    BI_RGB, DIB_RGB_COLORS, HGDIOBJ,
};
#[cfg(windows)]
use windows::Win32::Storage::FileSystem::FILE_ATTRIBUTE_NORMAL;
#[cfg(windows)]
use windows::Win32::UI::Shell::{SHGetFileInfoW, SHFILEINFOW, SHGFI_ICON, SHGFI_LARGEICON};
#[cfg(windows)]
use windows::Win32::UI::WindowsAndMessaging::{DestroyIcon, GetIconInfo, HICON, ICONINFO};

#[derive(Clone)]
pub struct RgbaIcon {
    pub width: usize,
    pub height: usize,
    pub rgba: Vec<u8>,
}

#[cfg(target_os = "linux")]
pub fn icon_for_exe(path:&str)->Option<RgbaIcon>{crate::desktop_linux::icon(path)}

#[cfg(not(any(windows,target_os = "linux")))]
pub fn icon_for_exe(_path: &str) -> Option<RgbaIcon> {
    None
}

#[cfg(windows)]
pub fn icon_for_exe(path: &str) -> Option<RgbaIcon> {
    if path.is_empty() {
        return None;
    }
    let wide: Vec<u16> = path.encode_utf16().chain(std::iter::once(0)).collect();
    unsafe {
        let mut sfi = SHFILEINFOW::default();
        let r = SHGetFileInfoW(
            PCWSTR(wide.as_ptr()),
            FILE_ATTRIBUTE_NORMAL,
            Some(&mut sfi),
            std::mem::size_of::<SHFILEINFOW>() as u32,
            SHGFI_ICON | SHGFI_LARGEICON,
        );
        if r == 0 || sfi.hIcon.is_invalid() {
            return None;
        }
        let out = hicon_to_rgba(sfi.hIcon);
        let _ = DestroyIcon(sfi.hIcon);
        out
    }
}

#[cfg(windows)]
unsafe fn hicon_to_rgba(hicon: HICON) -> Option<RgbaIcon> {
    let mut info = ICONINFO::default();
    GetIconInfo(hicon, &mut info).ok()?;
    let color = info.hbmColor;
    let mask = info.hbmMask;
    let result = (|| {
        let mut bm = BITMAP::default();
        let target = if color.is_invalid() { mask } else { color };
        if GetObjectW(HGDIOBJ(target.0), std::mem::size_of::<BITMAP>() as i32, Some(&mut bm as *mut _ as *mut c_void)) == 0 {
            return None;
        }
        let w = bm.bmWidth.max(1) as usize;
        let mut h = bm.bmHeight.max(1) as usize;
        if color.is_invalid() {
            h /= 2; // máscara monocromática: AND em cima, XOR embaixo
        }
        if w > 256 || h > 256 {
            return None;
        }
        let hdc = CreateCompatibleDC(None);
        if hdc.is_invalid() {
            return None;
        }
        let mut bmi = BITMAPINFO::default();
        bmi.bmiHeader = BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: w as i32,
            biHeight: -(h as i32),
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0,
            ..Default::default()
        };
        let mut px = vec![0u8; w * h * 4];
        let mut ok = false;
        if !color.is_invalid() {
            ok = GetDIBits(hdc, color, 0, h as u32, Some(px.as_mut_ptr() as *mut c_void), &mut bmi, DIB_RGB_COLORS) != 0;
        }
        // máscara (para ícones sem canal alfa)
        let mut mask_px = vec![0u8; w * h * 4];
        let mut have_mask = false;
        if !mask.is_invalid() {
            let mut bmi2 = bmi;
            bmi2.bmiHeader.biHeight = -(h as i32);
            have_mask = GetDIBits(hdc, mask, 0, h as u32, Some(mask_px.as_mut_ptr() as *mut c_void), &mut bmi2, DIB_RGB_COLORS) != 0;
        }
        let _ = DeleteDC(hdc);
        if !ok {
            return None;
        }
        // BGRA -> RGBA
        let has_alpha = px.chunks_exact(4).any(|c| c[3] != 0);
        for (i, c) in px.chunks_exact_mut(4).enumerate() {
            c.swap(0, 2);
            if !has_alpha {
                let transparent = have_mask && mask_px[i * 4] != 0;
                c[3] = if transparent { 0 } else { 255 };
            }
        }
        Some(RgbaIcon { width: w, height: h, rgba: px })
    })();
    if !color.is_invalid() {
        let _ = DeleteObject(HGDIOBJ(color.0));
    }
    if !mask.is_invalid() {
        let _ = DeleteObject(HGDIOBJ(mask.0));
    }
    result
}

// ---------- banco de ícones sob demanda ----------

use std::collections::{HashMap, HashSet};
use std::sync::mpsc::{channel, Receiver, Sender};

/// Cache de ícones por caminho, carregados numa thread à parte.
///
/// O `Snapshot` do sampler já traz o ícone de todo processo vivo, mas a Partida precisa do
/// ícone de coisa que *não* está rodando — e `SHGetFileInfoW` custa milissegundos por
/// arquivo. Numa lista de 366 entradas isso travaria a UI por meio segundo a cada refresh.
pub struct IconBank {
    tex: HashMap<String, Option<egui::TextureHandle>>,
    pending: HashSet<String>,
    req: Sender<String>,
    done: Receiver<(String, Option<RgbaIcon>)>,
}

impl IconBank {
    pub fn new() -> Self {
        let (req, req_rx) = channel::<String>();
        let (done_tx, done) = channel::<(String, Option<RgbaIcon>)>();
        std::thread::Builder::new()
            .name("ramdog-icons".into())
            .spawn(move || {
                init_com_for_shell();
                while let Ok(path) = req_rx.recv() {
                    let icon = icon_for_exe(&path);
                    if done_tx.send((path, icon)).is_err() {
                        return;
                    }
                }
            })
            .expect("spawn icon thread");
        Self { tex: HashMap::new(), pending: HashSet::new(), req, done }
    }

    /// Sobe para textura tudo que a thread devolveu desde o frame anterior.
    pub fn poll(&mut self, ctx: &egui::Context) -> bool {
        let mut got = false;
        while let Ok((path, icon)) = self.done.try_recv() {
            self.pending.remove(&path);
            let handle = icon.map(|ic| {
                let img = egui::ColorImage::from_rgba_unmultiplied([ic.width, ic.height], &ic.rgba);
                ctx.load_texture(format!("bank:{path}"), img, egui::TextureOptions::LINEAR)
            });
            self.tex.insert(path, handle);
            got = true;
        }
        got
    }

    /// Ícone do caminho, pedindo o carregamento na primeira vez que é perguntado.
    pub fn get(&mut self, path: &str) -> Option<egui::TextureHandle> {
        if let Some(t) = self.tex.get(path) {
            return t.clone();
        }
        if self.pending.insert(path.to_string()) {
            let _ = self.req.send(path.to_string());
        }
        None
    }
}

#[cfg(windows)]
fn init_com_for_shell() {
    use windows::Win32::System::Com::{CoInitializeEx, COINIT_APARTMENTTHREADED};
    unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
    }
}

#[cfg(not(windows))]
fn init_com_for_shell() {}
