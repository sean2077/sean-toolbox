//! Windows clipboard backend.
//!
//! A message-only window receives `WM_CLIPBOARDUPDATE`. Reads cover
//! CF_UNICODETEXT, CF_HTML, PNG/CF_DIB and CF_HDROP; writes set the matching
//! formats plus the `SeanToolbox.ClipOrigin` marker for loop suppression.

use crate::backend::{Backend, BackendEvent, ORIGIN_WINDOWS_FORMAT, ReadContent};
use crate::cf_html;
use crate::image_codec::{dib_to_png, png_to_dib};
use crate::model::ClipboardContent;
use std::{
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc::Sender,
    },
    thread,
};
use windows_sys::Win32::{
    Foundation::{GlobalFree, HGLOBAL, HMODULE, HWND, LPARAM, LRESULT, WPARAM},
    System::{
        DataExchange::{
            AddClipboardFormatListener, CloseClipboard, EmptyClipboard, GetClipboardData,
            IsClipboardFormatAvailable, OpenClipboard, RegisterClipboardFormatW,
            RemoveClipboardFormatListener, SetClipboardData,
        },
        Memory::{GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalSize, GlobalUnlock},
        Ole::{CF_DIB, CF_DIBV5, CF_HDROP, CF_UNICODETEXT, DROPEFFECT_COPY},
        Threading::Sleep,
    },
    UI::{
        Shell::{DROPFILES, DragQueryFileW, HDROP},
        WindowsAndMessaging::{
            CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, HWND_MESSAGE, MSG,
            PM_REMOVE, PeekMessageW, RegisterClassW, TranslateMessage, UnregisterClassW,
            WM_CLIPBOARDUPDATE, WNDCLASSW,
        },
    },
};

const HTML_FORMAT: &str = "HTML Format";
const PNG_FORMAT: &str = "PNG";
const PREFERRED_DROP_EFFECT: &str = "Preferred DropEffect";
const EXCLUDE_FORMAT: &str = "ExcludeClipboardContentFromMonitorProcessing";
const INCLUDE_HISTORY_FORMAT: &str = "CanIncludeInClipboardHistory";
const VIEWER_IGNORE_FORMAT: &str = "Clipboard Viewer Ignore";

pub fn backend() -> Result<Box<dyn Backend>, String> {
    Ok(Box::new(WindowsBackend::new()))
}

pub struct WindowsBackend {
    inner: Arc<Inner>,
    started: bool,
}

struct Inner {
    latest: Mutex<Option<ReadContent>>,
    pending_write: Mutex<Option<ClipboardContent>>,
    events: Mutex<Option<Sender<BackendEvent>>>,
    stop: AtomicBool,
}

impl WindowsBackend {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Inner {
                latest: Mutex::new(None),
                pending_write: Mutex::new(None),
                events: Mutex::new(None),
                stop: AtomicBool::new(false),
            }),
            started: false,
        }
    }
}

impl Default for WindowsBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for WindowsBackend {
    fn drop(&mut self) {
        self.inner.stop.store(true, Ordering::Release);
    }
}

impl Backend for WindowsBackend {
    fn watch(&mut self, events: Sender<BackendEvent>) -> Result<(), String> {
        if self.started {
            return Ok(());
        }
        self.started = true;
        *self.inner.events.lock().unwrap() = Some(events);
        let inner = self.inner.clone();
        thread::Builder::new()
            .name("toolbox-clipboard-win32".into())
            .spawn(move || {
                if let Err(error) = run(inner.clone()) {
                    eprintln!("Sean Toolbox clipboard (Windows): {error}");
                    if let Some(events) = inner.events.lock().unwrap().as_ref() {
                        let _ = events.send(BackendEvent::Unavailable);
                    }
                }
            })
            .map(|_| ())
            .map_err(|error| error.to_string())
    }

    fn read(&mut self) -> Result<ReadContent, String> {
        Ok(self
            .inner
            .latest
            .lock()
            .unwrap()
            .clone()
            .unwrap_or_default())
    }

    fn write(&mut self, content: &ClipboardContent) -> Result<(), String> {
        *self.inner.pending_write.lock().unwrap() = Some(content.clone());
        Ok(())
    }
}

fn run(inner: Arc<Inner>) -> Result<(), String> {
    let class_name = wide("SeanToolboxClipboard");
    let hinstance =
        unsafe { windows_sys::Win32::System::LibraryLoader::GetModuleHandleW(std::ptr::null()) };
    let window_class = WNDCLASSW {
        style: 0,
        lpfnWndProc: Some(wndproc),
        cbClsExtra: 0,
        cbWndExtra: 0,
        hInstance: hinstance as _,
        hIcon: std::ptr::null_mut(),
        hCursor: std::ptr::null_mut(),
        hbrBackground: std::ptr::null_mut(),
        lpszMenuName: std::ptr::null(),
        lpszClassName: class_name.as_ptr(),
    };
    let registered = unsafe { RegisterClassW(&window_class) };
    if registered == 0 {
        return Err("RegisterClassW failed".into());
    }
    let hwnd = unsafe {
        CreateWindowExW(
            0,
            class_name.as_ptr(),
            class_name.as_ptr(),
            0,
            0,
            0,
            0,
            0,
            HWND_MESSAGE,
            std::ptr::null_mut(),
            hinstance as _,
            std::ptr::null_mut(),
        )
    };
    if hwnd as isize == 0 {
        unsafe {
            UnregisterClassW(class_name.as_ptr(), hinstance as _);
        }
        return Err("CreateWindowExW failed".into());
    }
    if unsafe { AddClipboardFormatListener(hwnd) } == 0 {
        cleanup(hwnd, &class_name, hinstance);
        return Err("AddClipboardFormatListener failed".into());
    }
    while !inner.stop.load(Ordering::Acquire) {
        let pending = inner.pending_write.lock().unwrap().take();
        if let Some(content) = pending
            && let Err(error) = write_clipboard(hwnd, &content, &inner)
        {
            eprintln!("Sean Toolbox clipboard write: {error}");
        }
        let mut message: MSG = unsafe { std::mem::zeroed() };
        while unsafe { PeekMessageW(&mut message, std::ptr::null_mut(), 0, 0, PM_REMOVE) } != 0 {
            if message.message == WM_CLIPBOARDUPDATE {
                read_clipboard(hwnd, &inner);
            } else {
                unsafe {
                    TranslateMessage(&message);
                    DispatchMessageW(&message);
                }
            }
        }
        unsafe { Sleep(20) };
    }
    cleanup(hwnd, &class_name, hinstance);
    Ok(())
}

fn cleanup(hwnd: HWND, class_name: &[u16], hinstance: HMODULE) {
    unsafe {
        RemoveClipboardFormatListener(hwnd);
        DestroyWindow(hwnd);
        UnregisterClassW(class_name.as_ptr(), hinstance as _);
    }
}

unsafe extern "system" fn wndproc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    unsafe { DefWindowProcW(hwnd, message, wparam, lparam) }
}

fn notify(inner: &Arc<Inner>) {
    if let Some(events) = inner.events.lock().unwrap().as_ref() {
        let _ = events.send(BackendEvent::Changed);
    }
}

fn open_clipboard(hwnd: HWND) -> bool {
    for _ in 0..10 {
        if unsafe { OpenClipboard(hwnd) } != 0 {
            return true;
        }
        unsafe { Sleep(25) };
    }
    false
}

fn read_clipboard(hwnd: HWND, inner: &Arc<Inner>) {
    if !open_clipboard(hwnd) {
        eprintln!("Sean Toolbox clipboard: OpenClipboard stayed busy after 10 retries");
        return;
    }
    let read = read_locked();
    unsafe { CloseClipboard() };
    if let Some(read) = read {
        *inner.latest.lock().unwrap() = Some(read);
        notify(inner);
    }
}

fn read_locked() -> Option<ReadContent> {
    let origin = register(ORIGIN_WINDOWS_FORMAT);
    if origin != 0 && unsafe { IsClipboardFormatAvailable(origin) } != 0 {
        return Some(ReadContent {
            content: ClipboardContent::default(),
            remote_origin: true,
            sensitive: false,
        });
    }
    if is_sensitive() {
        return Some(ReadContent {
            content: ClipboardContent::default(),
            remote_origin: false,
            sensitive: true,
        });
    }
    let mut content = ClipboardContent::default();
    if let Some(bytes) = clipboard_bytes(CF_UNICODETEXT as u32) {
        content.text = decode_utf16(&bytes);
    }
    let html_format = register(HTML_FORMAT);
    if html_format != 0
        && let Some(bytes) = clipboard_bytes(html_format)
    {
        content.html = cf_html::parse(&bytes);
    }
    let png_format = register(PNG_FORMAT);
    if png_format != 0
        && let Some(bytes) = clipboard_bytes(png_format)
        && !bytes.is_empty()
    {
        content.image = Some(bytes);
    } else if let Some(bytes) =
        clipboard_bytes(CF_DIBV5 as u32).or_else(|| clipboard_bytes(CF_DIB as u32))
        && let Some(png) = dib_to_png(&bytes)
    {
        content.image = Some(png);
    }
    content.files = read_files();
    Some(ReadContent {
        content,
        remote_origin: false,
        sensitive: false,
    })
}

fn is_sensitive() -> bool {
    let exclude = register(EXCLUDE_FORMAT);
    if exclude != 0 && unsafe { IsClipboardFormatAvailable(exclude) } != 0 {
        return true;
    }
    let ignore = register(VIEWER_IGNORE_FORMAT);
    if ignore != 0 && unsafe { IsClipboardFormatAvailable(ignore) } != 0 {
        return true;
    }
    let history = register(INCLUDE_HISTORY_FORMAT);
    if history != 0 && unsafe { IsClipboardFormatAvailable(history) } != 0 {
        if let Some(bytes) = clipboard_bytes(history) {
            if bytes.len() >= 4 && u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) == 0
            {
                return true;
            }
        }
    }
    false
}

fn write_clipboard(
    hwnd: HWND,
    content: &ClipboardContent,
    inner: &Arc<Inner>,
) -> Result<(), String> {
    if !open_clipboard(hwnd) {
        return Err("OpenClipboard failed after 10 retries".into());
    }
    let result = write_locked(content);
    unsafe { CloseClipboard() };
    if result.is_ok() {
        *inner.latest.lock().unwrap() = Some(ReadContent {
            content: content.clone(),
            remote_origin: true,
            sensitive: false,
        });
        notify(inner);
    }
    result
}

fn write_locked(content: &ClipboardContent) -> Result<(), String> {
    if unsafe { EmptyClipboard() } == 0 {
        return Err("EmptyClipboard failed".into());
    }
    if let Some(text) = &content.text {
        let mut bytes = Vec::with_capacity(text.len() * 2 + 2);
        for unit in text.encode_utf16() {
            bytes.extend_from_slice(&unit.to_le_bytes());
        }
        bytes.extend_from_slice(&0u16.to_le_bytes());
        set_bytes(CF_UNICODETEXT as u32, &bytes)?;
    }
    if let Some(html) = &content.html {
        let format = register(HTML_FORMAT);
        if format == 0 {
            return Err("cannot register HTML Format".into());
        }
        set_bytes(format, &cf_html::build(html))?;
    }
    if let Some(image) = &content.image {
        let format = register(PNG_FORMAT);
        if format != 0 {
            set_bytes(format, image)?;
        }
        if let Some(dib) = png_to_dib(image) {
            set_bytes(CF_DIB as u32, &dib)?;
        }
    }
    if !content.files.is_empty() {
        set_bytes(CF_HDROP as u32, &dropfiles(&content.files)?)?;
        let format = register(PREFERRED_DROP_EFFECT);
        if format != 0 {
            set_bytes(format, &(DROPEFFECT_COPY as u32).to_le_bytes())?;
        }
    }
    let origin = register(ORIGIN_WINDOWS_FORMAT);
    if origin != 0 {
        let mut bytes = Vec::new();
        for unit in content.fingerprint().encode_utf16() {
            bytes.extend_from_slice(&unit.to_le_bytes());
        }
        bytes.extend_from_slice(&0u16.to_le_bytes());
        set_bytes(origin, &bytes)?;
    }
    Ok(())
}

fn set_bytes(format: u32, bytes: &[u8]) -> Result<(), String> {
    if bytes.is_empty() {
        return Ok(());
    }
    let handle = global_from_bytes(bytes)?;
    if unsafe { SetClipboardData(format, handle) } as isize == 0 {
        unsafe { GlobalFree(handle) };
        return Err("SetClipboardData failed".into());
    }
    Ok(())
}

fn global_from_bytes(bytes: &[u8]) -> Result<HGLOBAL, String> {
    let handle = unsafe { GlobalAlloc(GMEM_MOVEABLE, bytes.len()) };
    if handle as isize == 0 {
        return Err("GlobalAlloc failed".into());
    }
    let pointer = unsafe { GlobalLock(handle) };
    if pointer.is_null() {
        unsafe { GlobalFree(handle) };
        return Err("GlobalLock failed".into());
    }
    unsafe {
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), pointer.cast(), bytes.len());
        GlobalUnlock(handle);
    }
    Ok(handle)
}

fn clipboard_bytes(format: u32) -> Option<Vec<u8>> {
    let handle = unsafe { GetClipboardData(format) };
    if handle as isize == 0 {
        return None;
    }
    let pointer = unsafe { GlobalLock(handle) };
    if pointer.is_null() {
        return None;
    }
    let size = unsafe { GlobalSize(handle) };
    if size == 0 {
        unsafe { GlobalUnlock(handle) };
        return None;
    }
    let bytes = unsafe { std::slice::from_raw_parts(pointer.cast::<u8>(), size).to_vec() };
    unsafe { GlobalUnlock(handle) };
    Some(bytes)
}

fn decode_utf16(bytes: &[u8]) -> Option<String> {
    let units = bytes
        .chunks_exact(2)
        .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
        .take_while(|unit| *unit != 0)
        .collect::<Vec<_>>();
    if units.is_empty() {
        None
    } else {
        Some(String::from_utf16_lossy(&units))
    }
}

fn read_files() -> Vec<PathBuf> {
    let handle = unsafe { GetClipboardData(CF_HDROP as u32) };
    if handle as isize == 0 {
        return Vec::new();
    }
    let drop = handle as HDROP;
    let count = unsafe { DragQueryFileW(drop, u32::MAX, std::ptr::null_mut(), 0) };
    let mut files = Vec::with_capacity(count as usize);
    for index in 0..count {
        let length = unsafe { DragQueryFileW(drop, index, std::ptr::null_mut(), 0) };
        if length == 0 {
            continue;
        }
        let mut buffer = vec![0u16; length as usize + 1];
        let written =
            unsafe { DragQueryFileW(drop, index, buffer.as_mut_ptr(), buffer.len() as u32) };
        if written == 0 {
            continue;
        }
        buffer.truncate(written as usize);
        files.push(PathBuf::from(String::from_utf16_lossy(&buffer)));
    }
    files
}

fn dropfiles(files: &[PathBuf]) -> Result<Vec<u8>, String> {
    let mut header: DROPFILES = unsafe { std::mem::zeroed() };
    header.pFiles = std::mem::size_of::<DROPFILES>() as u32;
    header.fWide = 1;
    let mut bytes = unsafe {
        std::slice::from_raw_parts(
            (&header as *const DROPFILES).cast::<u8>(),
            std::mem::size_of::<DROPFILES>(),
        )
    }
    .to_vec();
    for file in files {
        for unit in file.to_string_lossy().encode_utf16() {
            bytes.extend_from_slice(&unit.to_le_bytes());
        }
        bytes.extend_from_slice(&0u16.to_le_bytes());
    }
    bytes.extend_from_slice(&0u16.to_le_bytes());
    Ok(bytes)
}

fn register(name: &str) -> u32 {
    let wide_name = wide(name);
    unsafe { RegisterClipboardFormatW(wide_name.as_ptr()) }
}

fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(Some(0)).collect()
}
