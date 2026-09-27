//! The Windows implementation: COM UI Automation + GDI screenshots.
//!
//! Everything `unsafe` in this package is contained here. Built only on
//! Windows (guarded by `#[cfg(windows)] mod win;` in `main.rs`).
//!
//! NOTE: this file targets the `windows` crate 0.58. It has not been compiled
//! on this workstation (no Windows toolchain here); a few handle wrappers
//! (`Option<HWND>` vs `HWND`) and constant paths may need a small adjustment on
//! the first Windows build — see `README` / CI. The shape and logic are
//! complete.

use std::ffi::c_void;
use std::sync::OnceLock;

use base64::Engine;
use serde_json::{json, Value};

use windows::core::{Interface, BSTR};
use windows::Win32::Foundation::{CloseHandle, BOOL, HANDLE, HWND, LPARAM, RECT};
use windows::Win32::Graphics::Gdi::{
    CreateCompatibleBitmap, CreateCompatibleDC, DeleteDC, DeleteObject, GetDIBits, GetWindowDC,
    ReleaseDC, SelectObject, BITMAPINFO, BITMAPINFOHEADER, DIB_RGB_COLORS, HGDIOBJ,
};
use windows::Win32::Storage::Xps::{PrintWindow, PRINT_WINDOW_FLAGS};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CLSCTX_ALL, COINIT_MULTITHREADED,
};
use windows::Win32::System::Threading::{
    OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
};
use windows::Win32::UI::Accessibility::{
    CUIAutomation, IUIAutomation, IUIAutomationElement, IUIAutomationInvokePattern,
    IUIAutomationTreeWalker, IUIAutomationValuePattern, UIA_InvokePatternId, UIA_ValuePatternId,
};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetForegroundWindow, GetWindowRect, GetWindowTextLengthW, GetWindowTextW,
    GetWindowThreadProcessId, IsWindow, IsWindowVisible,
};

/// `PW_RENDERFULLCONTENT` — capture GPU-composited content too.
const PW_RENDERFULLCONTENT: PRINT_WINDOW_FLAGS = PRINT_WINDOW_FLAGS(2);

use crate::{err_reply, Action, Match, Request, Target};

const MAX_DEPTH: usize = 12;
const MAX_NODES: usize = 1500;

/// A typed failure that maps onto the JSON error envelope.
enum HelperError {
    WindowGone,
    Failed(String),
}

impl HelperError {
    fn reply(self) -> Value {
        match self {
            HelperError::WindowGone => err_reply("window_gone", "the window closed or changed"),
            HelperError::Failed(m) => err_reply("failed", m),
        }
    }
}

impl From<windows::core::Error> for HelperError {
    fn from(e: windows::core::Error) -> Self {
        HelperError::Failed(e.message())
    }
}

fn failed(m: impl Into<String>) -> HelperError {
    HelperError::Failed(m.into())
}

/// Dispatch one command.
pub fn handle(request: Request) -> Value {
    ensure_com();
    let result = match request {
        Request::Resolve { matcher } => resolve(&matcher),
        Request::Capture { hwnd, pid, exe } => capture(hwnd, pid, &exe),
        Request::Act {
            hwnd,
            pid,
            exe,
            action,
        } => act(hwnd, pid, &exe, &action),
    };
    result.unwrap_or_else(HelperError::reply)
}

fn ensure_com() {
    static INIT: OnceLock<()> = OnceLock::new();
    INIT.get_or_init(|| {
        // Ignore the HRESULT: S_FALSE means already initialized on this thread,
        // which is fine for our single-threaded, short-lived process.
        let _ = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
    });
}

fn hwnd_from(raw: i64) -> HWND {
    HWND(raw as *mut c_void)
}

// ---------------------------------------------------------------------------
// Window identity
// ---------------------------------------------------------------------------

/// The owning process id and executable path of `hwnd`, or [`HelperError::WindowGone`]
/// if the window is gone.
fn owner(hwnd: HWND) -> Result<(u32, String), HelperError> {
    if !unsafe { IsWindow(hwnd) }.as_bool() {
        return Err(HelperError::WindowGone);
    }
    let mut pid = 0u32;
    unsafe { GetWindowThreadProcessId(hwnd, Some(&mut pid)) };
    if pid == 0 {
        return Err(HelperError::WindowGone);
    }
    let handle: HANDLE = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) }
        .map_err(|e| failed(format!("OpenProcess: {}", e.message())))?;
    let mut buf = [0u16; 512];
    let mut size = buf.len() as u32;
    let query = unsafe {
        QueryFullProcessImageNameW(
            handle,
            PROCESS_NAME_WIN32,
            windows::core::PWSTR(buf.as_mut_ptr()),
            &mut size,
        )
    };
    let _ = unsafe { CloseHandle(handle) };
    query.map_err(|e| failed(format!("QueryFullProcessImageNameW: {}", e.message())))?;
    let exe = String::from_utf16_lossy(&buf[..size as usize]);
    Ok((pid, exe))
}

fn window_title(hwnd: HWND) -> String {
    let len = unsafe { GetWindowTextLengthW(hwnd) };
    if len <= 0 {
        return String::new();
    }
    let mut buf = vec![0u16; len as usize + 1];
    let n = unsafe { GetWindowTextW(hwnd, &mut buf) };
    String::from_utf16_lossy(&buf[..n.max(0) as usize])
}

fn exe_name(exe: &str) -> String {
    exe.rsplit(['\\', '/']).next().unwrap_or(exe).to_owned()
}

fn window_id_value(hwnd: HWND) -> Result<Value, HelperError> {
    let (pid, exe) = owner(hwnd)?;
    Ok(json!({
        "hwnd": hwnd.0 as i64,
        "pid": pid,
        "exe": exe,
        "exe_name": exe_name(&exe),
        "title": window_title(hwnd),
    }))
}

/// Re-check that `hwnd` still exists and is owned by the same process and
/// executable the caller expects. Guards `capture` and `act` against acting on
/// a window that closed or was replaced (BMD-13, req 3).
fn verify_same_window(
    hwnd: HWND,
    expected_pid: u32,
    expected_exe: &str,
) -> Result<(), HelperError> {
    let (pid, exe) = owner(hwnd)?;
    if pid != expected_pid || !exe.eq_ignore_ascii_case(expected_exe) {
        return Err(HelperError::WindowGone);
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// resolve
// ---------------------------------------------------------------------------

fn resolve(matcher: &Match) -> Result<Value, HelperError> {
    let hwnd = if let Some(fragment) = &matcher.title_contains {
        find_window(|h| window_title(h).contains(fragment.as_str()))?
    } else if let Some(name) = &matcher.exe_name {
        let name = name.to_ascii_lowercase();
        find_window(|h| {
            owner(h)
                .map(|(_, exe)| exe_name(&exe).to_ascii_lowercase() == name)
                .unwrap_or(false)
        })?
    } else {
        let h = unsafe { GetForegroundWindow() };
        if h.0.is_null() {
            return Err(failed("no foreground window"));
        }
        h
    };
    Ok(json!({ "ok": true, "window": window_id_value(hwnd)? }))
}

/// The first visible top-level window matching `pred`.
fn find_window<F: Fn(HWND) -> bool>(pred: F) -> Result<HWND, HelperError> {
    let mut windows: Vec<HWND> = Vec::new();
    unsafe {
        EnumWindows(
            Some(enum_proc),
            LPARAM(&mut windows as *mut Vec<HWND> as isize),
        )
    }
    .map_err(|e| failed(format!("EnumWindows: {}", e.message())))?;

    windows
        .into_iter()
        .filter(|h| unsafe { IsWindowVisible(*h) }.as_bool())
        .find(|h| pred(*h))
        .ok_or_else(|| failed("no window matched"))
}

unsafe extern "system" fn enum_proc(hwnd: HWND, lparam: LPARAM) -> BOOL {
    let list = &mut *(lparam.0 as *mut Vec<HWND>);
    list.push(hwnd);
    true.into()
}

// ---------------------------------------------------------------------------
// capture
// ---------------------------------------------------------------------------

fn capture(hwnd_raw: i64, pid: u32, exe: &str) -> Result<Value, HelperError> {
    let hwnd = hwnd_from(hwnd_raw);
    verify_same_window(hwnd, pid, exe)?;
    let automation: IUIAutomation = unsafe { CoCreateInstance(&CUIAutomation, None, CLSCTX_ALL) }?;
    let root = unsafe { automation.ElementFromHandle(hwnd) }?;
    let walker = unsafe { automation.ControlViewWalker() }?;
    let mut count = 0usize;
    let tree = build_node(&walker, &root, 0, "0", &mut count)?;
    let png = screenshot(hwnd)?;
    Ok(json!({
        "ok": true,
        "window": window_id_value(hwnd)?,
        "tree": tree,
        "png_base64": png,
    }))
}

fn element_json(element: &IUIAutomationElement, reference: &str) -> Value {
    let name = unsafe { element.CurrentName() }
        .map(|b| b.to_string())
        .unwrap_or_default();
    let automation_id = unsafe { element.CurrentAutomationId() }
        .map(|b| b.to_string())
        .unwrap_or_default();
    let control_type = unsafe { element.CurrentControlType() }
        .map(|t| t.0)
        .unwrap_or(0);
    let rect = unsafe { element.CurrentBoundingRectangle() }.unwrap_or(RECT::default());
    json!({
        "ref": reference,
        "name": name,
        "automation_id": automation_id,
        "control_type": control_type,
        "rect": [rect.left, rect.top, rect.right, rect.bottom],
    })
}

fn build_node(
    walker: &IUIAutomationTreeWalker,
    element: &IUIAutomationElement,
    depth: usize,
    reference: &str,
    count: &mut usize,
) -> Result<Value, HelperError> {
    let mut node = element_json(element, reference);
    let mut children = Vec::new();
    if depth < MAX_DEPTH && *count < MAX_NODES {
        // A null child/sibling comes back as Err; treat that as "none".
        let mut child = unsafe { walker.GetFirstChildElement(element) }.ok();
        let mut index = 0usize;
        while let Some(current) = child {
            *count += 1;
            if *count >= MAX_NODES {
                break;
            }
            let child_ref = format!("{reference}.{index}");
            children.push(build_node(walker, &current, depth + 1, &child_ref, count)?);
            child = unsafe { walker.GetNextSiblingElement(&current) }.ok();
            index += 1;
        }
    }
    node["children"] = Value::Array(children);
    Ok(node)
}

// ---------------------------------------------------------------------------
// screenshot (GDI)
// ---------------------------------------------------------------------------

fn screenshot(hwnd: HWND) -> Result<String, HelperError> {
    let mut rect = RECT::default();
    unsafe { GetWindowRect(hwnd, &mut rect) }
        .map_err(|e| failed(format!("GetWindowRect: {}", e.message())))?;
    let width = rect.right - rect.left;
    let height = rect.bottom - rect.top;
    if width <= 0 || height <= 0 || width > 16384 || height > 16384 {
        return Err(failed(format!(
            "window has no usable size ({width}x{height})"
        )));
    }

    unsafe {
        let hdc_window = GetWindowDC(hwnd);
        if hdc_window.is_invalid() {
            return Err(failed("GetWindowDC failed"));
        }
        let hdc_mem = CreateCompatibleDC(hdc_window);
        let hbitmap = CreateCompatibleBitmap(hdc_window, width, height);
        let old = SelectObject(hdc_mem, HGDIOBJ(hbitmap.0));

        // PW_RENDERFULLCONTENT (0x2) captures GPU-composited content too.
        let ok = PrintWindow(hwnd, hdc_mem, PW_RENDERFULLCONTENT).as_bool();

        // Top-down 32bpp BGRA.
        let mut info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: width,
                biHeight: -height,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: 0, // BI_RGB
                ..Default::default()
            },
            ..Default::default()
        };
        let mut buffer = vec![0u8; (width as usize) * (height as usize) * 4];
        let lines = GetDIBits(
            hdc_mem,
            hbitmap,
            0,
            height as u32,
            Some(buffer.as_mut_ptr() as *mut c_void),
            &mut info,
            DIB_RGB_COLORS,
        );

        SelectObject(hdc_mem, old);
        let _ = DeleteObject(HGDIOBJ(hbitmap.0));
        let _ = DeleteDC(hdc_mem);
        ReleaseDC(hwnd, hdc_window);

        if !ok || lines == 0 {
            return Err(failed("PrintWindow/GetDIBits captured nothing"));
        }

        // BGRA -> RGBA (step by 4 rather than chunks_exact_mut, which clippy
        // flags for a constant chunk size).
        let mut i = 0;
        while i + 3 < buffer.len() {
            buffer.swap(i, i + 2);
            buffer[i + 3] = 0xFF;
            i += 4;
        }
        encode_png(width as u32, height as u32, buffer)
    }
}

fn encode_png(width: u32, height: u32, rgba: Vec<u8>) -> Result<String, HelperError> {
    let image = image::RgbaImage::from_raw(width, height, rgba)
        .ok_or_else(|| failed("screenshot buffer size mismatch"))?;
    let mut png = std::io::Cursor::new(Vec::new());
    image
        .write_to(&mut png, image::ImageFormat::Png)
        .map_err(|e| failed(format!("PNG encode: {e}")))?;
    Ok(base64::engine::general_purpose::STANDARD.encode(png.into_inner()))
}

// ---------------------------------------------------------------------------
// act
// ---------------------------------------------------------------------------

fn act(hwnd_raw: i64, pid: u32, exe: &str, action: &Action) -> Result<Value, HelperError> {
    let hwnd = hwnd_from(hwnd_raw);
    // BMD-13 / req 3: never act unless this is still the same window.
    verify_same_window(hwnd, pid, exe)?;
    let automation: IUIAutomation = unsafe { CoCreateInstance(&CUIAutomation, None, CLSCTX_ALL) }?;
    let root = unsafe { automation.ElementFromHandle(hwnd) }?;
    let walker = unsafe { automation.ControlViewWalker() }?;
    let target = find_target(&walker, &root, &action.target)?;

    match action.kind.as_str() {
        "invoke" => {
            let pattern = unsafe { target.GetCurrentPattern(UIA_InvokePatternId) }?;
            let invoke: IUIAutomationInvokePattern = pattern.cast()?;
            unsafe { invoke.Invoke() }?;
            Ok(json!({"ok": true, "result": {"invoked": true}}))
        }
        "set_value" => {
            let value = action
                .value
                .as_deref()
                .ok_or_else(|| failed("set_value needs a value"))?;
            let pattern = unsafe { target.GetCurrentPattern(UIA_ValuePatternId) }?;
            let value_pattern: IUIAutomationValuePattern = pattern.cast()?;
            unsafe { value_pattern.SetValue(&BSTR::from(value)) }?;
            Ok(json!({"ok": true, "result": {"set_value": value}}))
        }
        other => Err(failed(format!("unknown action kind {other:?}"))),
    }
}

/// Find the element to act on: by dotted `ref` path, else by automation id,
/// else by name. First match wins.
fn find_target(
    walker: &IUIAutomationTreeWalker,
    root: &IUIAutomationElement,
    target: &Target,
) -> Result<IUIAutomationElement, HelperError> {
    if let Some(path) = &target.reference {
        return follow_ref(walker, root, path);
    }
    if let Some(id) = &target.automation_id {
        return find_first(walker, root, 0, &|e| {
            unsafe { e.CurrentAutomationId() }
                .ok()
                .map(|b| b.to_string())
                .as_deref()
                == Some(id.as_str())
        })
        .ok_or_else(|| failed(format!("no element with automation_id {id:?}")));
    }
    if let Some(name) = &target.name {
        return find_first(walker, root, 0, &|e| {
            unsafe { e.CurrentName() }
                .ok()
                .map(|b| b.to_string())
                .as_deref()
                == Some(name.as_str())
        })
        .ok_or_else(|| failed(format!("no element named {name:?}")));
    }
    Err(failed("action target is empty"))
}

fn follow_ref(
    walker: &IUIAutomationTreeWalker,
    root: &IUIAutomationElement,
    path: &str,
) -> Result<IUIAutomationElement, HelperError> {
    // Path is "0" (root) then child indices: "0.2.1".
    let mut parts = path.split('.');
    if parts.next() != Some("0") {
        return Err(failed(format!("bad ref {path:?}")));
    }
    let mut element = root.clone();
    for part in parts {
        let want: usize = part
            .parse()
            .map_err(|_| failed(format!("bad ref {path:?}")))?;
        let mut child = unsafe { walker.GetFirstChildElement(&element) }.ok();
        let mut index = 0usize;
        loop {
            let current = child.ok_or(HelperError::WindowGone)?;
            if index == want {
                element = current;
                break;
            }
            child = unsafe { walker.GetNextSiblingElement(&current) }.ok();
            index += 1;
        }
    }
    Ok(element)
}

fn find_first(
    walker: &IUIAutomationTreeWalker,
    element: &IUIAutomationElement,
    depth: usize,
    pred: &dyn Fn(&IUIAutomationElement) -> bool,
) -> Option<IUIAutomationElement> {
    if pred(element) {
        return Some(element.clone());
    }
    if depth >= MAX_DEPTH {
        return None;
    }
    let mut child = unsafe { walker.GetFirstChildElement(element) }.ok();
    while let Some(current) = child {
        if let Some(found) = find_first(walker, &current, depth + 1, pred) {
            return Some(found);
        }
        child = unsafe { walker.GetNextSiblingElement(&current) }.ok();
    }
    None
}
