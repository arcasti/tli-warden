//! Captures the game: finds its window by its process, else by a part of the
//! title, and copies its client area with Windows Graphics Capture, which sees DirectX and OpenGL
//! windows. Where that isn't available it falls back to a copy from the screen
//! (BitBlt), which needs the window in view and misses flip-model swapchains.
//! [`Watcher`] keeps one capture session open for auto read.

use super::session::{Watch, fingerprint};
use crate::reader::Rgba;
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, channel};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};
use windows::Graphics::Capture::{Direct3D11CaptureFrame, Direct3D11CaptureFramePool, GraphicsCaptureItem, GraphicsCaptureSession};
use windows::Graphics::DirectX::Direct3D11::IDirect3DDevice;
use windows::Graphics::SizeInt32;
use windows::Graphics::DirectX::DirectXPixelFormat;
use windows::Win32::Foundation::{CloseHandle, HMODULE, HWND, LPARAM, POINT, RECT};
use windows::Win32::Graphics::Direct3D::D3D_DRIVER_TYPE_HARDWARE;
use windows::Win32::Graphics::Direct3D11::{
    D3D11_CPU_ACCESS_READ, D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_MAP_READ, D3D11_MAPPED_SUBRESOURCE, D3D11_SDK_VERSION,
    D3D11_TEXTURE2D_DESC, D3D11_USAGE_STAGING, D3D11CreateDevice, ID3D11Device, ID3D11DeviceContext, ID3D11Texture2D,
};
use windows::Win32::Graphics::Dwm::{DWMWA_EXTENDED_FRAME_BOUNDS, DwmGetWindowAttribute};
use windows::Win32::Graphics::Dxgi::IDXGIDevice;
use windows::Win32::Graphics::Gdi::{
    BI_RGB, BITMAPINFO, BITMAPINFOHEADER, BitBlt, ClientToScreen, CreateCompatibleBitmap, CreateCompatibleDC, DIB_RGB_COLORS, DeleteDC,
    DeleteObject, GetDC, GetDIBits, ReleaseDC, SRCCOPY, SelectObject,
};
use windows::Win32::System::Threading::{
    GetCurrentProcessId, OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW,
};
use windows::Win32::System::WinRT::Direct3D11::{CreateDirect3D11DeviceFromDXGIDevice, IDirect3DDxgiInterfaceAccess};
use windows::Win32::System::WinRT::Graphics::Capture::IGraphicsCaptureItemInterop;
use windows::Win32::UI::HiDpi::{DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetThreadDpiAwarenessContext};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetClassNameW, GetClientRect, GetWindowTextW, GetWindowThreadProcessId, IsIconic, IsWindowVisible,
};
use windows::core::{BOOL, Interface, PWSTR, factory};

/// How long to wait for Windows Graphics Capture to deliver a frame.
const FRAME_TIMEOUT: Duration = Duration::from_secs(1);
/// How often the watcher samples the game.
const SAMPLE: Duration = Duration::from_millis(250);
/// How long the watcher waits after a failure before looking for the window again.
const RETRY: Duration = Duration::from_secs(1);
/// Frames the capture pool holds.
const BUFFERS: i32 = 2;
const FORMAT: DirectXPixelFormat = DirectXPixelFormat::B8G8R8A8UIntNormalized;
/// Console host windows: their title is the command line, which can contain the searched text.
const CONSOLE_CLASSES: [&str; 2] = ["ConsoleWindowClass", "CASCADIA_HOSTING_WINDOW_CLASS"];
/// The game's executable: its window is the one captured, whatever other windows' titles hold.
const GAME_EXE: &str = "torchlight_infinite.exe";

struct Search {
    needle: String,
    /// A window of [`GAME_EXE`] ends the search.
    game: Option<HWND>,
    /// The first window titled exactly `needle`.
    exact: Option<HWND>,
    /// The first window whose title contains `needle`.
    partial: Option<HWND>,
}

unsafe extern "system" fn visit(hwnd: HWND, lparam: LPARAM) -> BOOL {
    // SAFETY: lparam is the &mut Search passed to EnumWindows below, alive for the call.
    let search = unsafe { &mut *(lparam.0 as *mut Search) };
    let title = window_title(hwnd).to_lowercase();
    // SAFETY: hwnd comes from EnumWindows.
    if title.is_empty() || !unsafe { IsWindowVisible(hwnd) }.as_bool() {
        return BOOL(1);
    }
    let pid = window_pid(hwnd);
    if own_or_console(hwnd, pid) {
        return BOOL(1);
    }
    if process_file(pid).is_some_and(|f| f.eq_ignore_ascii_case(GAME_EXE)) {
        search.game = Some(hwnd);
        return BOOL(0);
    }
    if title == search.needle {
        search.exact.get_or_insert(hwnd);
    } else if title.contains(&search.needle) {
        search.partial.get_or_insert(hwnd);
    }
    BOOL(1)
}

/// The id of the process that owns the window.
fn window_pid(hwnd: HWND) -> u32 {
    let mut pid = 0u32;
    // SAFETY: hwnd comes from EnumWindows; pid is a valid out pointer.
    unsafe { GetWindowThreadProcessId(hwnd, Some(&mut pid)) };
    pid
}

/// Whether the window belongs to this process or is a console host.
fn own_or_console(hwnd: HWND, pid: u32) -> bool {
    let mut class = [0u16; 64];
    // SAFETY: hwnd comes from EnumWindows; class is a valid buffer.
    let (len, own) = unsafe { (GetClassNameW(hwnd, &mut class), pid == GetCurrentProcessId()) };
    let class = String::from_utf16_lossy(&class[..len.max(0) as usize]);
    own || CONSOLE_CLASSES.contains(&class.as_str())
}

/// The file name of process `pid`'s executable, such as "notepad.exe";
/// none when the process can't be queried.
fn process_file(pid: u32) -> Option<String> {
    let mut buf = [0u16; 1024];
    let mut len = buf.len() as u32;
    // SAFETY: the handle is opened here and closed before returning; buf and len
    // are a valid buffer and its length in characters.
    let path = unsafe {
        let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let named = QueryFullProcessImageNameW(process, PROCESS_NAME_WIN32, PWSTR(buf.as_mut_ptr()), &mut len);
        let _ = CloseHandle(process);
        named.ok()?;
        String::from_utf16_lossy(&buf[..len as usize])
    };
    path.rsplit('\\').next().map(str::to_string)
}

/// The visible top-level window of [`GAME_EXE`], else the first titled
/// `title` (any case), else the first whose title contains it, leaving out
/// this process's windows and console windows.
fn find(title: &str) -> Option<HWND> {
    let mut search = Search { needle: title.to_lowercase(), game: None, exact: None, partial: None };
    // SAFETY: the callback only touches `search`, which outlives the call. EnumWindows
    // reports an error when the callback stops early, which is how the game's window ends it.
    let _ = unsafe { EnumWindows(Some(visit), LPARAM(&mut search as *mut Search as isize)) };
    search.game.or(search.exact).or(search.partial)
}

/// The window `find` picks for `title`, or why there is none.
fn find_window(title: &str) -> Result<HWND, String> {
    find(title).ok_or_else(|| {
        format!("No Torchlight: Infinite window ({GAME_EXE}), and no window with \"{title}\" in its title: is the game running?")
    })
}

/// The window's title; empty when it has none.
fn window_title(hwnd: HWND) -> String {
    let mut buf = [0u16; 256];
    // SAFETY: hwnd is a window handle from EnumWindows or a capture; buf is a valid buffer.
    let len = unsafe { GetWindowTextW(hwnd, &mut buf) };
    String::from_utf16_lossy(&buf[..len.max(0) as usize])
}

/// The window's title and client size, as "\"Title\" (1920 x 1080)".
fn describe(hwnd: HWND) -> String {
    let title = window_title(hwnd);
    let (w, h) = client_rect(hwnd).map_or((0, 0), |r| (r.right - r.left, r.bottom - r.top));
    format!("\"{}\" ({} x {})", title.trim(), w, h)
}

/// Runs `f` with this thread aware of per-monitor DPI, so window rectangles
/// and the screen copy come in physical pixels, as the captured frame does.
fn dpi_aware<T>(f: impl FnOnce() -> T) -> T {
    // SAFETY: sets this thread's DPI awareness and restores it.
    unsafe {
        let old = SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        let result = f();
        if !old.is_invalid() {
            SetThreadDpiAwarenessContext(old);
        }
        result
    }
}

/// The client area of the window `find` picks for `title`,
/// or the reason it can't be captured.
pub fn capture(title: &str) -> Result<Rgba, String> {
    let hwnd = find_window(title)?;
    dpi_aware(|| capture_window(hwnd))
}

/// The game's client area in physical screen pixels: left, top, width, height.
pub fn game_rect(title: &str) -> Result<[i32; 4], String> {
    let hwnd = find_window(title)?;
    let r = dpi_aware(|| client_rect(hwnd))?;
    Ok([r.left, r.top, r.right - r.left, r.bottom - r.top])
}

fn capture_window(hwnd: HWND) -> Result<Rgba, String> {
    let client = client_rect(hwnd)?;
    match first_frame(hwnd) {
        Ok(img) => Ok(img),
        Err(wgc) => screen_copy(client).map_err(|gdi| format!("Windows Graphics Capture failed ({wgc}); the screen copy failed too ({gdi}).")),
    }
}

/// The window's client area in screen coordinates, or why it can't be captured.
fn client_rect(hwnd: HWND) -> Result<RECT, String> {
    // SAFETY: plain Win32 calls on a window handle from `find`.
    let (client, origin) = unsafe {
        if IsIconic(hwnd).as_bool() {
            return Err("The window is minimized.".to_string());
        }
        let mut client = RECT::default();
        GetClientRect(hwnd, &mut client).map_err(|e| format!("GetClientRect: {e}"))?;
        let mut origin = POINT::default();
        let _ = ClientToScreen(hwnd, &mut origin);
        (client, origin)
    };
    let (w, h) = (client.right - client.left, client.bottom - client.top);
    if w <= 0 || h <= 0 {
        return Err("The window has no client area.".to_string());
    }
    Ok(RECT { left: origin.x, top: origin.y, right: origin.x + w, bottom: origin.y + h })
}

/// The first frame of a new Windows Graphics Capture session on the window.
fn first_frame(hwnd: HWND) -> Result<Rgba, String> {
    Stream::open(hwnd)?.next_frame()
}

/// A Windows Graphics Capture session on one window, kept open between frames.
struct Stream {
    hwnd: HWND,
    device: ID3D11Device,
    context: ID3D11DeviceContext,
    winrt_device: IDirect3DDevice,
    _item: GraphicsCaptureItem,
    pool: Direct3D11CaptureFramePool,
    session: GraphicsCaptureSession,
    /// The frame size the pool was made for.
    size: SizeInt32,
    /// The texture frames are copied to for the CPU, reused while the size holds.
    staging: Option<(ID3D11Texture2D, u32, u32)>,
}

impl Stream {
    fn open(hwnd: HWND) -> Result<Stream, String> {
        if !GraphicsCaptureSession::IsSupported().unwrap_or(false) {
            return Err("not supported on this system".to_string());
        }
        // SAFETY: COM and Direct3D calls on objects created here.
        unsafe {
            let (mut device, mut context): (Option<ID3D11Device>, Option<ID3D11DeviceContext>) = (None, None);
            D3D11CreateDevice(
                None,
                D3D_DRIVER_TYPE_HARDWARE,
                HMODULE::default(),
                D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                None,
                D3D11_SDK_VERSION,
                Some(&mut device),
                None,
                Some(&mut context),
            )
            .map_err(fail("D3D11CreateDevice"))?;
            let (device, context) = (device.ok_or("no Direct3D device")?, context.ok_or("no Direct3D context")?);
            let dxgi: IDXGIDevice = device.cast().map_err(fail("IDXGIDevice"))?;
            let winrt_device: IDirect3DDevice =
                CreateDirect3D11DeviceFromDXGIDevice(&dxgi).and_then(|d| d.cast()).map_err(fail("CreateDirect3D11DeviceFromDXGIDevice"))?;

            let interop = factory::<GraphicsCaptureItem, IGraphicsCaptureItemInterop>().map_err(fail("capture interop"))?;
            let item: GraphicsCaptureItem = interop.CreateForWindow(hwnd).map_err(fail("CreateForWindow"))?;
            let size = item.Size().map_err(fail("item size"))?;
            let pool = Direct3D11CaptureFramePool::CreateFreeThreaded(&winrt_device, FORMAT, BUFFERS, size).map_err(fail("CreateFreeThreaded"))?;
            let session = pool.CreateCaptureSession(&item).map_err(fail("CreateCaptureSession"))?;
            // Windows 11 only; older systems keep the border and the cursor.
            let _ = session.SetIsBorderRequired(false);
            let _ = session.SetIsCursorCaptureEnabled(false);
            session.StartCapture().map_err(fail("StartCapture"))?;
            Ok(Stream { hwnd, device, context, winrt_device, _item: item, pool, session, size, staging: None })
        }
    }

    /// The newest frame since the last call, cropped to the client area;
    /// none when no frame came. Fails when the window is gone or minimized.
    fn grab(&mut self) -> Result<Option<Rgba>, String> {
        let client = client_rect(self.hwnd)?;
        let mut newest = None;
        while let Ok(frame) = self.pool.TryGetNextFrame() {
            if let Some(old) = newest.replace(frame) {
                let _ = old.Close();
            }
        }
        let Some(frame) = newest else { return Ok(None) };
        let result = self.copy(&frame, client);
        let size = frame.ContentSize();
        let _ = frame.Close();
        let size = size.map_err(fail("frame size"))?;
        if (size.Width, size.Height) != (self.size.Width, self.size.Height) {
            self.pool.Recreate(&self.winrt_device, FORMAT, BUFFERS, size).map_err(fail("Recreate"))?;
            self.size = size;
        }
        result.map(Some)
    }

    /// The frame's pixels under `client` (screen coordinates in physical pixels).
    fn copy(&mut self, frame: &Direct3D11CaptureFrame, client: RECT) -> Result<Rgba, String> {
        // SAFETY: COM and Direct3D calls on objects owned here; the mapped pointer is
        // read within the staging texture's row pitch and height, then unmapped.
        unsafe {
            // Where the frame sits on screen: the window without its invisible resize borders.
            let mut frame_rect = RECT::default();
            DwmGetWindowAttribute(self.hwnd, DWMWA_EXTENDED_FRAME_BOUNDS, (&mut frame_rect as *mut RECT).cast(), size_of::<RECT>() as u32)
                .map_err(fail("DwmGetWindowAttribute"))?;
            let texture: ID3D11Texture2D = frame
                .Surface()
                .and_then(|s| s.cast::<IDirect3DDxgiInterfaceAccess>())
                .and_then(|a| a.GetInterface())
                .map_err(fail("frame texture"))?;
            let mut desc = D3D11_TEXTURE2D_DESC::default();
            texture.GetDesc(&mut desc);
            let staging = match &self.staging {
                Some((t, w, h)) if (*w, *h) == (desc.Width, desc.Height) => t.clone(),
                _ => {
                    let (w, h) = (desc.Width, desc.Height);
                    desc.Usage = D3D11_USAGE_STAGING;
                    desc.BindFlags = 0;
                    desc.CPUAccessFlags = D3D11_CPU_ACCESS_READ.0 as u32;
                    desc.MiscFlags = 0;
                    let mut staging = None;
                    self.device.CreateTexture2D(&desc, None, Some(&mut staging)).map_err(fail("staging texture"))?;
                    let staging = staging.ok_or("no staging texture")?;
                    self.staging = Some((staging.clone(), w, h));
                    staging
                }
            };
            self.context.CopyResource(&staging, &texture);

            let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
            self.context.Map(&staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped)).map_err(fail("Map"))?;
            // The client area inside the frame, clipped to it.
            let (fw, fh) = (desc.Width as i32, desc.Height as i32);
            let x0 = (client.left - frame_rect.left).clamp(0, fw);
            let y0 = (client.top - frame_rect.top).clamp(0, fh);
            let x1 = (client.right - frame_rect.left).clamp(x0, fw);
            let y1 = (client.bottom - frame_rect.top).clamp(y0, fh);
            let (w, h) = ((x1 - x0) as usize, (y1 - y0) as usize);
            let pitch = mapped.RowPitch as usize;
            let src = std::slice::from_raw_parts(mapped.pData as *const u8, pitch * fh as usize);
            let mut data = Vec::with_capacity(w * h * 4);
            for y in y0 as usize..y1 as usize {
                let row = &src[y * pitch + x0 as usize * 4..y * pitch + x1 as usize * 4];
                for px in row.as_chunks::<4>().0 {
                    data.extend_from_slice(&[px[2], px[1], px[0], 255]);
                }
            }
            self.context.Unmap(&staging, 0);
            if w == 0 || h == 0 {
                return Err("the frame doesn't cover the client area".to_string());
            }
            Ok(Rgba { w, h, data })
        }
    }

    /// Waits for the next frame, up to [`FRAME_TIMEOUT`].
    fn next_frame(&mut self) -> Result<Rgba, String> {
        let start = Instant::now();
        loop {
            if let Some(img) = self.grab()? {
                return Ok(img);
            }
            if start.elapsed() > FRAME_TIMEOUT {
                return Err("no frame within 1 s".to_string());
            }
            std::thread::sleep(Duration::from_millis(2));
        }
    }
}

impl Drop for Stream {
    fn drop(&mut self) {
        let _ = self.session.Close();
        let _ = self.pool.Close();
    }
}

/// Grabs `n` frames of the window through one capture session, as the
/// watcher does, waiting 50 ms between them: the last frame and how long
/// each grab and fingerprint took (the first includes opening the session).
pub fn time_frames(title: &str, n: usize) -> Result<(Rgba, Vec<Duration>), String> {
    let hwnd = find_window(title)?;
    dpi_aware(|| {
        let mut start = Instant::now();
        let mut stream = Stream::open(hwnd)?;
        let mut times = Vec::new();
        loop {
            let frame = stream.next_frame()?;
            fingerprint(&frame);
            times.push(start.elapsed());
            if times.len() >= n.max(1) {
                return Ok((frame, times));
            }
            std::thread::sleep(Duration::from_millis(50));
            start = Instant::now();
        }
    })
}

/// What the watcher reports.
pub enum Event {
    /// A settled new screen to read.
    Frame(Rgba),
    /// Why the watcher can't see the game, or none once it can again.
    Status(Option<String>),
    /// The window a new capture session opened on, as its title and client size.
    Window(String),
}

/// The watcher's state the UI thread reaches too.
struct Shared {
    watch: Watch,
    /// The newest sampled frame and its fingerprint.
    latest: Option<(Rgba, Vec<u8>)>,
}

/// Auto read: a thread that keeps a capture session on the game open, samples
/// it every [`SAMPLE`] and sends a frame when [`Watch`] says to read it.
/// Dropping it stops the thread.
pub struct Watcher {
    /// Dropped to stop the thread.
    stop: Option<Sender<()>>,
    thread: Option<JoinHandle<()>>,
    events: Receiver<Event>,
    shared: Arc<Mutex<Shared>>,
}

impl Watcher {
    /// Watches the window `find` picks for `title`; `notify` runs after each event.
    pub fn start(title: &str, notify: impl Fn() + Send + 'static) -> Watcher {
        let (stop, stopped) = channel();
        let (tx, events) = channel();
        let shared = Arc::new(Mutex::new(Shared { watch: Watch::default(), latest: None }));
        let (title, state) = (title.to_string(), shared.clone());
        let thread = std::thread::spawn(move || dpi_aware(|| watch(&title, &stopped, &tx, &state, &notify)));
        Watcher { stop: Some(stop), thread: Some(thread), events, shared }
    }

    /// The next event, if any.
    pub fn event(&self) -> Option<Event> {
        self.events.try_recv().ok()
    }

    /// The newest sampled frame, counted as read so auto read leaves it.
    pub fn read_latest(&self) -> Option<Rgba> {
        let mut shared = lock(&self.shared);
        let (frame, print) = shared.latest.clone()?;
        shared.watch.mark_read(print);
        Some(frame)
    }

    /// Counts the next sample as read: an opened screenshot stays on show
    /// until the game's screen changes.
    pub fn adopt_next(&self) {
        lock(&self.shared).watch.adopt_next();
    }

    /// Reads the next sample whatever it shows: a held read asks for one more.
    pub fn read_next(&self) {
        lock(&self.shared).watch.read_next();
    }
}

impl Drop for Watcher {
    fn drop(&mut self) {
        self.stop.take();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn lock(shared: &Mutex<Shared>) -> MutexGuard<'_, Shared> {
    shared.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The watcher thread: samples until `stopped` disconnects; after a failure
/// drops the session and looks for the window again every [`RETRY`].
fn watch(title: &str, stopped: &Receiver<()>, tx: &Sender<Event>, shared: &Mutex<Shared>, notify: &dyn Fn()) {
    let send = |event| {
        if tx.send(event).is_ok() {
            notify();
        }
    };
    let (mut stream, mut status) = (None, None);
    loop {
        let opening = stream.is_none();
        let (wait, now) = match sample(title, &mut stream, shared) {
            Ok(read) => {
                if opening && let Some(s) = &stream {
                    send(Event::Window(describe(s.hwnd)));
                }
                if let Some(frame) = read {
                    send(Event::Frame(frame));
                }
                (SAMPLE, None)
            }
            Err(e) => {
                stream = None;
                lock(shared).latest = None;
                (RETRY, Some(e))
            }
        };
        if now != status {
            status = now;
            send(Event::Status(status.clone()));
        }
        if !matches!(stopped.recv_timeout(wait), Err(RecvTimeoutError::Timeout)) {
            return;
        }
    }
}

/// One sample: opens the session when there is none, takes the newest frame
/// (or the last one again when the game showed nothing new) and feeds its
/// print to the [`Watch`]; the frame when it should be read.
fn sample(title: &str, stream: &mut Option<Stream>, shared: &Mutex<Shared>) -> Result<Option<Rgba>, String> {
    let s = match stream {
        Some(s) => s,
        None => stream.insert(Stream::open(find_window(title)?)?),
    };
    let fresh = s.grab()?.map(|frame| {
        let print = fingerprint(&frame);
        (frame, print)
    });
    let mut shared = lock(shared);
    let Some((frame, print)) = fresh.or_else(|| shared.latest.take()) else { return Ok(None) };
    let read = shared.watch.sample(print.clone()).then(|| frame.clone());
    shared.latest = Some((frame, print));
    Ok(read)
}

/// The screen pixels under `client` (screen coordinates); the window has to be in view.
fn screen_copy(client: RECT) -> Result<Rgba, String> {
    let (w, h) = (client.right - client.left, client.bottom - client.top);
    // SAFETY: GDI objects created here and released before returning.
    unsafe {
        let screen = GetDC(None);
        let dc = CreateCompatibleDC(Some(screen));
        let bitmap = CreateCompatibleBitmap(screen, w, h);
        let old = SelectObject(dc, bitmap.into());
        let blit = BitBlt(dc, 0, 0, w, h, Some(screen), client.left, client.top, SRCCOPY);
        SelectObject(dc, old);

        let mut info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: w,
                biHeight: -h,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut data = vec![0u8; w as usize * h as usize * 4];
        let lines = GetDIBits(dc, bitmap, 0, h as u32, Some(data.as_mut_ptr().cast()), &mut info, DIB_RGB_COLORS);

        let _ = DeleteObject(bitmap.into());
        let _ = DeleteDC(dc);
        ReleaseDC(None, screen);
        blit.map_err(|e| format!("BitBlt: {e}"))?;
        if lines != h {
            return Err("GetDIBits failed".to_string());
        }
        for px in data.as_chunks_mut::<4>().0 {
            px.swap(0, 2);
            px[3] = 255;
        }
        Ok(Rgba { w: w as usize, h: h as usize, data })
    }
}

/// Maps a Windows error to "what: message".
fn fail(what: &'static str) -> impl Fn(windows::core::Error) -> String {
    move |e| format!("{what}: {}", e.message())
}
