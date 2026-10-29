//! Recuadro sutil alrededor del monitor que se graba.
//!
//! Es una ventana transparente, sin clics y siempre encima, marcada con
//! `WDA_EXCLUDEFROMCAPTURE`: se ve en pantalla pero no sale en la grabación.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::Duration;
use windows::core::w;
use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{BeginPaint, CreateSolidBrush, DeleteObject, EndPaint, FillRect, PAINTSTRUCT};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::HiDpi::{SetThreadDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2};
use windows::Win32::UI::WindowsAndMessaging::*;

const KEY: COLORREF = COLORREF(0x0001_0001); // color que se vuelve transparente
const RED: COLORREF = COLORREF(0x004d_48e5); // #e5484d (BGR)
const THICKNESS: i32 = 3;
const ALPHA: u8 = 190;

pub struct Overlay {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Overlay {
    pub fn show(monitor: u32) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let flag = stop.clone();
        let thread = thread::spawn(move || unsafe { run(monitor, &flag) });
        Self { stop, thread: Some(thread) }
    }
}

impl Drop for Overlay {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

unsafe fn run(monitor: u32, stop: &AtomicBool) {
    SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    let Some(m) = crate::monitors().into_iter().find(|m| m.index == monitor) else { return };
    let Ok(module) = GetModuleHandleW(None) else { return };
    let class = w!("GrabadorOverlay");
    RegisterClassW(&WNDCLASSW { lpfnWndProc: Some(wndproc), hInstance: module.into(), lpszClassName: class, ..Default::default() });

    let Ok(hwnd) = CreateWindowExW(
        WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
        class,
        w!("Grabador"),
        WS_POPUP,
        m.x,
        m.y,
        m.width as i32,
        m.height as i32,
        None,
        None,
        Some(module.into()),
        None,
    ) else {
        return;
    };
    let _ = SetLayeredWindowAttributes(hwnd, KEY, ALPHA, LWA_COLORKEY | LWA_ALPHA);
    let _ = SetWindowDisplayAffinity(hwnd, WDA_EXCLUDEFROMCAPTURE);
    let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);

    let mut msg = MSG::default();
    while !stop.load(Ordering::Relaxed) {
        while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
        thread::sleep(Duration::from_millis(50));
    }
    let _ = DestroyWindow(hwnd);
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    if msg != WM_PAINT {
        return DefWindowProcW(hwnd, msg, wp, lp);
    }
    let mut ps = PAINTSTRUCT::default();
    let hdc = BeginPaint(hwnd, &mut ps);
    let mut rc = RECT::default();
    let _ = GetClientRect(hwnd, &mut rc);
    let (w, h, t) = (rc.right, rc.bottom, THICKNESS);

    let key = CreateSolidBrush(KEY);
    FillRect(hdc, &rc, key);
    let red = CreateSolidBrush(RED);
    for strip in [
        RECT { left: 0, top: 0, right: w, bottom: t },
        RECT { left: 0, top: h - t, right: w, bottom: h },
        RECT { left: 0, top: 0, right: t, bottom: h },
        RECT { left: w - t, top: 0, right: w, bottom: h },
    ] {
        FillRect(hdc, &strip, red);
    }
    let _ = DeleteObject(key.into());
    let _ = DeleteObject(red.into());
    let _ = EndPaint(hwnd, &ps);
    LRESULT(0)
}
