//! Windows: a native layered window (WS_EX_LAYERED) that is topmost, click-
//! through (WS_EX_TRANSPARENT), never activated and kept off the taskbar. Each
//! redraw renders the dial with the shared cairo renderer into an ARGB32 image
//! at the monitor's DPI and hands it to UpdateLayeredWindow, whose premultiplied
//! BGRA format is exactly cairo's. Created on the GTK main thread; GLib's main
//! loop pumps its (few) messages.

use crate::config::Dial;
use crate::dial::{self, View, PAD};
use crate::platform::Placement;
use gtk::{cairo, glib};
use gtk4 as gtk;
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{COLORREF, HWND, POINT, SIZE};
use windows::Win32::Graphics::Gdi::{
    CreateCompatibleDC, CreateDIBSection, DeleteDC, DeleteObject, GetDC, ReleaseDC, SelectObject, AC_SRC_ALPHA,
    AC_SRC_OVER, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, BLENDFUNCTION, DIB_RGB_COLORS,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, RegisterClassW, SetWindowPos, ShowWindow, UpdateLayeredWindow, HWND_TOPMOST,
    SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SW_HIDE, SW_SHOWNOACTIVATE, ULW_ALPHA, WNDCLASSW, WS_EX_LAYERED,
    WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_EX_TRANSPARENT, WS_POPUP,
};

unsafe extern "system" fn wndproc(
    hwnd: HWND,
    msg: u32,
    wp: windows::Win32::Foundation::WPARAM,
    lp: windows::Win32::Foundation::LPARAM,
) -> windows::Win32::Foundation::LRESULT {
    unsafe { DefWindowProcW(hwnd, msg, wp, lp) }
}

pub struct Surface {
    hwnd: HWND,
    view: Rc<RefCell<View>>,
    place: Rc<RefCell<Placement>>,
    shown: Rc<Cell<bool>>,
}

impl Surface {
    pub fn new(_app: &gtk::Application, _dial: &Dial, view: Rc<RefCell<View>>) -> Self {
        let hwnd = unsafe {
            let hinst = GetModuleHandleW(PCWSTR::null()).expect("module handle");
            let class = w!("tp-dial-overlay");
            let wc = WNDCLASSW {
                lpfnWndProc: Some(wndproc),
                hInstance: hinst.into(),
                lpszClassName: class,
                ..Default::default()
            };
            RegisterClassW(&wc);
            CreateWindowExW(
                WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
                class,
                w!("tp-dial"),
                WS_POPUP,
                0,
                0,
                1,
                1,
                None,
                None,
                Some(hinst.into()),
                None,
            )
            .expect("create overlay window")
        };
        Self { hwnd, view, place: Rc::new(RefCell::new(Placement::default())), shown: Rc::new(Cell::new(false)) }
    }

    pub fn place(&self, _dial: &Dial, place: &Placement) {
        *self.place.borrow_mut() = place.clone();
    }

    pub fn redraw(&self) {
        render(self.hwnd, &self.view.borrow(), &self.place.borrow(), &self.shown);
    }

    /// ~60fps timer while the opening animation or a GIF is running.
    pub fn start_ticks(&self, ticking: Rc<Cell<bool>>) {
        let (hwnd, view, place, shown) = (self.hwnd, self.view.clone(), self.place.clone(), self.shown.clone());
        glib::timeout_add_local(std::time::Duration::from_millis(16), move || {
            let (redraw, keep) = dial::tick(&mut view.borrow_mut(), glib::monotonic_time());
            if redraw {
                render(hwnd, &view.borrow(), &place.borrow(), &shown);
            }
            if keep {
                glib::ControlFlow::Continue
            } else {
                ticking.set(false);
                glib::ControlFlow::Break
            }
        });
    }
}

fn render(hwnd: HWND, v: &View, place: &Placement, shown: &Cell<bool>) {
    let Some(d) = &v.dial else {
        if shown.replace(false) {
            unsafe {
                let _ = ShowWindow(hwnd, SW_HIDE);
            }
        }
        return;
    };
    let scale = if place.scale > 0.0 { place.scale } else { 1.0 };
    let side = d.size + 2.0 * PAD; // logical px
    let px = (side * scale).ceil() as i32;
    let Ok(mut surf) = cairo::ImageSurface::create(cairo::Format::ARgb32, px, px) else { return };
    surf.set_device_scale(scale, scale);
    {
        let Ok(cr) = cairo::Context::new(&surf) else { return };
        dial::draw(v, &cr, side, side);
    }
    surf.flush();
    let stride = surf.stride() as usize;
    let Ok(data) = surf.data() else { return };

    // Keep the whole dial on the monitor's work area.
    let (l, t, r, b) = place.work;
    let clamp = |c: i32, lo: i32, hi: i32| if hi - lo >= px { c.clamp(lo, hi - px) } else { lo };
    let pos = POINT { x: clamp(place.center.0 - px / 2, l, r), y: clamp(place.center.1 - px / 2, t, b) };

    unsafe {
        let screen = GetDC(None);
        let mem = CreateCompatibleDC(Some(screen));
        let bmi = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: px,
                biHeight: -px, // top-down, like cairo
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut bits: *mut core::ffi::c_void = std::ptr::null_mut();
        if let Ok(bmp) = CreateDIBSection(Some(mem), &bmi, DIB_RGB_COLORS, &mut bits, None, 0) {
            let row = px as usize * 4;
            for y in 0..px as usize {
                std::ptr::copy_nonoverlapping(data.as_ptr().add(y * stride), (bits as *mut u8).add(y * row), row);
            }
            let old = SelectObject(mem, bmp.into());
            let size = SIZE { cx: px, cy: px };
            let src = POINT { x: 0, y: 0 };
            let blend = BLENDFUNCTION {
                BlendOp: AC_SRC_OVER as u8,
                BlendFlags: 0,
                SourceConstantAlpha: 255,
                AlphaFormat: AC_SRC_ALPHA as u8,
            };
            let _ = UpdateLayeredWindow(
                hwnd,
                Some(screen),
                Some(&pos),
                Some(&size),
                Some(mem),
                Some(&src),
                COLORREF(0),
                Some(&blend),
                ULW_ALPHA,
            );
            SelectObject(mem, old);
            let _ = DeleteObject(bmp.into());
        }
        let _ = DeleteDC(mem);
        ReleaseDC(None, screen);
        if !shown.replace(true) {
            let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
            let _ = SetWindowPos(hwnd, Some(HWND_TOPMOST), 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
        }
    }
}
