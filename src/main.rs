use clap::Parser;
use libc::{
    EPOLL_CLOEXEC, EPOLL_CTL_ADD, EPOLLIN, IN_CLOEXEC, IN_CREATE, IN_DELETE, IN_MOVED_FROM,
    IN_MOVED_TO, MAP_SHARED, MFD_CLOEXEC, O_NONBLOCK, PROT_READ, PROT_WRITE, c_void, epoll_create1,
    epoll_ctl, epoll_event, epoll_wait, ftruncate, inotify_add_watch, inotify_init1, memfd_create,
    mmap, munmap, read,
};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    fs::{self, File, OpenOptions},
    io::{self, Read, Write},
    os::fd::{AsFd, AsRawFd, FromRawFd, RawFd},
    os::unix::net::UnixStream,
    path::{Path, PathBuf},
};
use wayland_client::{
    Connection, Dispatch, EventQueue, QueueHandle,
    globals::{GlobalListContents, registry_queue_init},
    protocol::{
        wl_buffer::WlBuffer, wl_compositor::WlCompositor, wl_output::WlOutput, wl_region::WlRegion,
        wl_shm::WlShm, wl_shm_pool::WlShmPool, wl_surface::WlSurface,
    },
};
use wayland_protocols_wlr::layer_shell::v1::client::{
    zwlr_layer_shell_v1::{self, ZwlrLayerShellV1},
    zwlr_layer_surface_v1::{self, ZwlrLayerSurfaceV1},
};

const VERSION: &str = "0.6.0";
const MAX_NSD_LINE: usize = 16 * 1024;
const MAX_NSD_BUFFER: usize = 64 * 1024;
const ICON_SIZE: i32 = 64;

#[derive(Parser, Debug)]
#[command(name = "ldicons", version = VERSION, about = "Resource-friendly Wayland desktop icons")]
struct Args {
    #[arg(short = 'c', long, default_value_os_t = default_config())]
    config: PathBuf,
    #[arg(short = 'p', long, default_value_os_t = default_positions())]
    positions: PathBuf,
    #[arg(short = 'l', long)]
    log: Option<PathBuf>,
    #[arg(long)]
    verbose: bool,
    #[arg(long, help = "Scan the desktop and exit without connecting to Wayland")]
    check: bool,
}

fn home_path(suffix: &str) -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
        .join(suffix)
}
fn default_config() -> PathBuf {
    home_path(".config/ldicons/ldicons.conf")
}
fn default_positions() -> PathBuf {
    home_path(".config/ldicons/icon_positions.json")
}

#[derive(Clone, Debug)]
struct Config {
    desktop: PathBuf,
    show_hidden: bool,
    grid_x: i32,
    grid_y: i32,
    offset_x: i32,
    offset_y: i32,
    nsd_enabled: bool,
    nsd_socket: PathBuf,
    file_manager: String,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            desktop: std::env::var_os("XDG_DESKTOP_DIR")
                .map(PathBuf::from)
                .unwrap_or_else(|| home_path("Desktop")),
            show_hidden: true,
            grid_x: 140,
            grid_y: 180,
            offset_x: 50,
            offset_y: 50,
            nsd_enabled: true,
            nsd_socket: PathBuf::from("/tmp/nsd.sock"),
            file_manager: "xdg-open".into(),
        }
    }
}

impl Config {
    fn load(path: &Path) -> io::Result<Self> {
        let mut cfg = Self::default();
        let text = fs::read_to_string(path)?;
        let mut section = String::new();
        for raw in text.lines() {
            let line = raw.trim();
            if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
                continue;
            }
            if let Some(name) = line.strip_prefix('[').and_then(|v| v.strip_suffix(']')) {
                section = name.to_ascii_lowercase();
                continue;
            }
            let Some((key, value)) = line.split_once('=') else {
                continue;
            };
            let key = key.trim().to_ascii_lowercase();
            let value = value.trim().trim_matches('"');
            match (section.as_str(), key.as_str()) {
                ("layout", "grid_spacing_x") => cfg.grid_x = value.parse().unwrap_or(cfg.grid_x),
                ("layout", "grid_spacing_y") => cfg.grid_y = value.parse().unwrap_or(cfg.grid_y),
                ("layout", "start_offset_x") => {
                    cfg.offset_x = value.parse().unwrap_or(cfg.offset_x)
                }
                ("layout", "start_offset_y") => {
                    cfg.offset_y = value.parse().unwrap_or(cfg.offset_y)
                }
                ("behavior", "show_hidden_files") => {
                    cfg.show_hidden = value.eq_ignore_ascii_case("true")
                }
                ("daemon", "enabled") => cfg.nsd_enabled = value.eq_ignore_ascii_case("true"),
                ("daemon", "socket_path") => cfg.nsd_socket = expand_path(value),
                ("daemon", "file_manager") => cfg.file_manager = value.to_owned(),
                _ => {}
            }
        }
        if let Ok(desktop) = std::env::var("XDG_DESKTOP_DIR") {
            cfg.desktop = expand_path(&desktop);
        }
        Ok(cfg)
    }
}

fn expand_path(value: &str) -> PathBuf {
    if let Some(rest) = value.strip_prefix("~/") {
        home_path(rest)
    } else {
        PathBuf::from(value)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct Position {
    x: i32,
    y: i32,
}

#[derive(Clone, Debug)]
struct Icon {
    path: PathBuf,
    position: Position,
}

fn load_positions(path: &Path) -> HashMap<String, Position> {
    fs::read_to_string(path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

fn save_positions(path: &Path, icons: &[Icon]) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let positions: HashMap<_, _> = icons
        .iter()
        .map(|i| (i.path.to_string_lossy().into_owned(), i.position.clone()))
        .collect();
    let data = serde_json::to_vec_pretty(&positions).map_err(io::Error::other)?;
    let tmp = path.with_extension("json.tmp");
    fs::write(&tmp, data)?;
    fs::rename(tmp, path)
}

fn scan_desktop(config: &Config, positions: &HashMap<String, Position>) -> io::Result<Vec<Icon>> {
    let mut entries = fs::read_dir(&config.desktop)?
        .filter_map(Result::ok)
        .filter(|entry| config.show_hidden || !entry.file_name().to_string_lossy().starts_with('.'))
        .collect::<Vec<_>>();
    entries.sort_by_cached_key(|e| e.file_name());
    Ok(entries
        .into_iter()
        .enumerate()
        .map(|(index, entry)| {
            let path = entry.path();
            let key = path.to_string_lossy().into_owned();
            let position = positions.get(&key).cloned().unwrap_or_else(|| Position {
                x: config.offset_x + (index as i32 / 6) * config.grid_x,
                y: config.offset_y + (index as i32 % 6) * config.grid_y,
            });
            Icon {
                path,
                position,
            }
        })
        .collect())
}

struct NsdClient {
    stream: UnixStream,
    buffer: Vec<u8>,
}
impl NsdClient {
    fn connect(path: &Path) -> io::Result<Self> {
        let stream = UnixStream::connect(path)?;
        stream.set_nonblocking(true)?;
        Ok(Self {
            stream,
            buffer: Vec::with_capacity(4096),
        })
    }
    fn fd(&self) -> RawFd {
        self.stream.as_raw_fd()
    }
    fn read_events(&mut self) -> Vec<serde_json::Value> {
        let mut chunk = [0u8; 4096];
        loop {
            match (&self.stream).read(&mut chunk) {
                Ok(0) => break,
                Err(ref e) if e.kind() == io::ErrorKind::WouldBlock => break,
                Ok(n) => {
                    if self.buffer.len() + n > MAX_NSD_BUFFER {
                        self.buffer.clear();
                        break;
                    }
                    self.buffer.extend_from_slice(&chunk[..n]);
                }
                Err(_) => break,
            }
        }
        let mut result = Vec::new();
        while let Some(end) = self.buffer.iter().position(|b| *b == b'\n') {
            let line = self.buffer.drain(..=end).collect::<Vec<_>>();
            if line.len() > MAX_NSD_LINE {
                continue;
            }
            if let Ok(value) = serde_json::from_slice::<serde_json::Value>(&line[..line.len() - 1])
            {
                result.push(value);
            }
        }
        result
    }
}

struct LinuxWatch {
    fd: RawFd,
    _dir: PathBuf,
}
impl LinuxWatch {
    fn new(path: &Path) -> io::Result<Self> {
        let fd = unsafe { inotify_init1(IN_CLOEXEC | O_NONBLOCK) };
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        let watch_path =
            std::ffi::CString::new(path.as_os_str().as_encoded_bytes()).map_err(|_| {
                io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!("watch path contains NUL byte: {}", path.display()),
                )
            })?;
        let wd = unsafe {
            inotify_add_watch(
                fd,
                watch_path.as_ptr(),
                IN_CREATE | IN_DELETE | IN_MOVED_FROM | IN_MOVED_TO,
            )
        };
        if wd < 0 {
            unsafe { libc::close(fd) };
            return Err(io::Error::last_os_error());
        }
        Ok(Self {
            fd,
            _dir: path.to_owned(),
        })
    }
    fn drain(&self) -> bool {
        let mut buf = [0u8; 4096];
        unsafe { read(self.fd, buf.as_mut_ptr() as *mut c_void, buf.len()) > 0 }
    }
}
impl Drop for LinuxWatch {
    fn drop(&mut self) {
        unsafe {
            libc::close(self.fd);
        }
    }
}

struct App {
    config: Config,
    positions_path: PathBuf,
    icons: Vec<Icon>,
    watch: LinuxWatch,
    nsd: Option<NsdClient>,
}
impl App {
    fn new(args: &Args) -> io::Result<Self> {
        let config = Config::load(&args.config).unwrap_or_default();
        fs::create_dir_all(&config.desktop)?;
        let positions = load_positions(&args.positions);
        let icons = scan_desktop(&config, &positions)?;
        let watch = LinuxWatch::new(&config.desktop)?;
        let nsd = (config.nsd_enabled && config.nsd_socket.exists())
            .then(|| NsdClient::connect(&config.nsd_socket).ok())
            .flatten();
        Ok(Self {
            config,
            positions_path: args.positions.clone(),
            icons,
            watch,
            nsd,
        })
    }
    fn refresh(&mut self) -> io::Result<()> {
        let positions = load_positions(&self.positions_path);
        self.icons = scan_desktop(&self.config, &positions)?;
        Ok(())
    }
}

#[derive(Default)]
struct WaylandState {
    compositor: Option<WlCompositor>,
    shm: Option<WlShm>,
    layer_shell: Option<ZwlrLayerShellV1>,
    surface: Option<WlSurface>,
    layer_surface: Option<ZwlrLayerSurfaceV1>,
    buffer: Option<WlBuffer>,
    pool: Option<WlShmPool>,
    backing: Option<File>,
    icons: Vec<Position>,
    configured: bool,
}

impl Dispatch<wayland_client::protocol::wl_registry::WlRegistry, GlobalListContents>
    for WaylandState
{
    fn event(
        _: &mut Self,
        _: &wayland_client::protocol::wl_registry::WlRegistry,
        _: wayland_client::protocol::wl_registry::Event,
        _: &GlobalListContents,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}
impl Dispatch<WlCompositor, ()> for WaylandState {
    fn event(
        _: &mut Self,
        _: &WlCompositor,
        _: wayland_client::protocol::wl_compositor::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}
impl Dispatch<WlShm, ()> for WaylandState {
    fn event(
        _: &mut Self,
        _: &WlShm,
        _: wayland_client::protocol::wl_shm::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}
impl Dispatch<WlRegion, ()> for WaylandState {
    fn event(
        _: &mut Self,
        _: &WlRegion,
        _: wayland_client::protocol::wl_region::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}
impl Dispatch<WlOutput, ()> for WaylandState {
    fn event(
        _: &mut Self,
        _: &WlOutput,
        _: wayland_client::protocol::wl_output::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}
impl Dispatch<WlSurface, ()> for WaylandState {
    fn event(
        _: &mut Self,
        _: &WlSurface,
        _: wayland_client::protocol::wl_surface::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}
impl Dispatch<ZwlrLayerShellV1, ()> for WaylandState {
    fn event(
        _: &mut Self,
        _: &ZwlrLayerShellV1,
        _: zwlr_layer_shell_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}
impl Dispatch<WlBuffer, ()> for WaylandState {
    fn event(
        _: &mut Self,
        _: &WlBuffer,
        _: wayland_client::protocol::wl_buffer::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}
impl Dispatch<ZwlrLayerSurfaceV1, ()> for WaylandState {
    fn event(
        state: &mut Self,
        proxy: &ZwlrLayerSurfaceV1,
        event: zwlr_layer_surface_v1::Event,
        _: &(),
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        if let zwlr_layer_surface_v1::Event::Configure {
            serial,
            width,
            height,
        } = event
        {
            proxy.ack_configure(serial);
            state.configured = true;
            if state.buffer.is_none()
                && width > 0
                && height > 0
                && let Err(error) = render_memfd(state, width as i32, height as i32, qh)
            {
                eprintln!("ldicons: unable to render Wayland buffer: {error}");
            }
        }
    }
}
impl Dispatch<WlShmPool, ()> for WaylandState {
    fn event(
        _: &mut Self,
        _: &WlShmPool,
        _: wayland_client::protocol::wl_shm_pool::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

fn render_memfd(
    state: &mut WaylandState,
    width: i32,
    height: i32,
    qh: &QueueHandle<WaylandState>,
) -> io::Result<()> {
    let Some(shm) = state.shm.as_ref() else {
        return Ok(());
    };
    let size = i64::from(width.max(1)) * i64::from(height.max(1)) * 4;
    let name = std::ffi::CString::new("ldicons-frame").unwrap();
    let fd = unsafe { memfd_create(name.as_ptr(), MFD_CLOEXEC) };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    if unsafe { ftruncate(fd, size) } < 0 {
        unsafe { libc::close(fd) };
        return Err(io::Error::last_os_error());
    }
    let mapped = unsafe {
        mmap(
            std::ptr::null_mut(),
            size as usize,
            PROT_READ | PROT_WRITE,
            MAP_SHARED,
            fd,
            0,
        )
    };
    if mapped == libc::MAP_FAILED {
        unsafe { libc::close(fd) };
        return Err(io::Error::last_os_error());
    }
    let pixels = unsafe { std::slice::from_raw_parts_mut(mapped as *mut u32, (size / 4) as usize) };
    pixels.fill(0);
    let max_x = (width - ICON_SIZE).max(0);
    let max_y = (height - ICON_SIZE).max(0);
    for (index, position) in state.icons.iter().enumerate() {
        let x = position.x.max(0).min(max_x);
        let y = position.y.max(0).min(max_y);
        let color = 0xff4f86c6u32.wrapping_add((index as u32 & 0x3f) << 16);
        for row in y..(y + ICON_SIZE).min(height) {
            let start = (row * width + x) as usize;
            let end = (row * width + (x + ICON_SIZE).min(width)) as usize;
            pixels[start..end].fill(color);
        }
    }
    unsafe {
        munmap(mapped, size as usize);
    }
    let backing = unsafe { File::from_raw_fd(fd) };
    let pool = shm.create_pool(backing.as_fd(), size as i32, qh, ());
    let buffer = pool.create_buffer(
        0,
        width,
        height,
        width * 4,
        wayland_client::protocol::wl_shm::Format::Argb8888,
        qh,
        (),
    );
    if let Some(surface) = &state.surface {
        surface.attach(Some(&buffer), 0, 0);
        surface.damage_buffer(0, 0, width, height);
        surface.commit();
    }
    state.backing = Some(backing);
    state.pool = Some(pool);
    state.buffer = Some(buffer);
    Ok(())
}

fn create_layer_surface(
    conn: &Connection,
    icons: Vec<Position>,
) -> io::Result<(EventQueue<WaylandState>, WaylandState)> {
    let (globals, queue) =
        registry_queue_init(conn).map_err(|e| io::Error::other(e.to_string()))?;
    let mut state = WaylandState {
        icons,
        ..WaylandState::default()
    };
    let qh = queue.handle();
    state.compositor = Some(
        globals
            .bind::<WlCompositor, _, _>(&qh, 1..=4, ())
            .map_err(|e| io::Error::other(e.to_string()))?,
    );
    state.shm = Some(
        globals
            .bind::<WlShm, _, _>(&qh, 1..=1, ())
            .map_err(|e| io::Error::other(e.to_string()))?,
    );
    state.layer_shell = Some(
        globals
            .bind::<ZwlrLayerShellV1, _, _>(&qh, 1..=4, ())
            .map_err(|e| io::Error::other(e.to_string()))?,
    );
    let surface = state.compositor.as_ref().unwrap().create_surface(&qh, ());
    let layer = state.layer_shell.as_ref().unwrap().get_layer_surface(
        &surface,
        None,
        zwlr_layer_shell_v1::Layer::Background,
        "ldicons".into(),
        &qh,
        (),
    );
    layer.set_anchor(
        zwlr_layer_surface_v1::Anchor::Top
            | zwlr_layer_surface_v1::Anchor::Bottom
            | zwlr_layer_surface_v1::Anchor::Left
            | zwlr_layer_surface_v1::Anchor::Right,
    );
    layer.set_exclusive_zone(-1);
    layer.set_keyboard_interactivity(zwlr_layer_surface_v1::KeyboardInteractivity::None);
    surface.commit();
    state.surface = Some(surface);
    state.layer_surface = Some(layer);
    conn.flush().map_err(|e| io::Error::other(e.to_string()))?;
    Ok((queue, state))
}

fn epoll_add(epoll: RawFd, fd: RawFd) -> io::Result<()> {
    let mut event = epoll_event {
        events: EPOLLIN as u32,
        u64: fd as u64,
    };
    if unsafe { epoll_ctl(epoll, EPOLL_CTL_ADD, fd, &mut event) } < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

fn run(mut app: App, args: &Args) -> io::Result<()> {
    if args.check {
        println!(
            "{} desktop entries in {}",
            app.icons.len(),
            app.config.desktop.display()
        );
        return Ok(());
    }
    let conn = Connection::connect_to_env()
        .map_err(|e| io::Error::other(format!("Wayland connection failed: {e}")))?;
    let (mut queue, mut state) = create_layer_surface(
        &conn,
        app.icons.iter().map(|icon| icon.position.clone()).collect(),
    )?;
    let epoll = unsafe { epoll_create1(EPOLL_CLOEXEC) };
    if epoll < 0 {
        return Err(io::Error::last_os_error());
    }
    let wayland_fd = conn.backend().poll_fd().as_raw_fd();
    epoll_add(epoll, wayland_fd)?;
    epoll_add(epoll, app.watch.fd)?;
    if let Some(nsd) = &app.nsd {
        epoll_add(epoll, nsd.fd())?;
    }
    let mut events = [epoll_event { events: 0, u64: 0 }; 8];
    loop {
        let count = unsafe { epoll_wait(epoll, events.as_mut_ptr(), events.len() as i32, -1) };
        if count < 0 && io::Error::last_os_error().kind() != io::ErrorKind::Interrupted {
            return Err(io::Error::last_os_error());
        }
        for event in events.iter().take(count.max(0) as usize) {
            let fd = event.u64 as RawFd;
            if fd == app.watch.fd && app.watch.drain() {
                app.refresh()?;
                save_positions(&app.positions_path, &app.icons)?;
            }
            if let Some(nsd) = &mut app.nsd
                && fd == nsd.fd()
            {
                for msg in nsd.read_events() {
                    if args.verbose {
                        eprintln!("[nsd] {msg}");
                    }
                }
            }
            if fd == wayland_fd {
                queue
                    .blocking_dispatch(&mut state)
                    .map_err(|e| io::Error::other(e.to_string()))?;
            }
        }
        conn.flush().ok();
    }
}

fn main() -> io::Result<()> {
    let args = Args::parse();
    if let Some(log) = &args.log {
        if let Some(parent) = log.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut file = OpenOptions::new().create(true).append(true).open(log)?;
        writeln!(file, "ldicons {VERSION} starting")?;
    }
    let app = App::new(&args)?;
    run(app, &args)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expands_home_paths() {
        let path = expand_path("~/Desktop");
        assert!(path.ends_with("Desktop"));
        assert!(!path.to_string_lossy().starts_with("~/"));
    }

    #[test]
    fn positions_round_trip_as_json() {
        let mut positions = HashMap::new();
        positions.insert("/tmp/example".to_owned(), Position { x: 50, y: 180 });
        let encoded = serde_json::to_string(&positions).unwrap();
        let decoded: HashMap<String, Position> = serde_json::from_str(&encoded).unwrap();
        assert_eq!(decoded["/tmp/example"].x, 50);
        assert_eq!(decoded["/tmp/example"].y, 180);
    }

    #[test]
    fn nsd_lines_are_bounded() {
        fn line_limit() -> usize {
            MAX_NSD_LINE
        }
        fn buffer_limit() -> usize {
            MAX_NSD_BUFFER
        }

        assert!(line_limit() < buffer_limit());
        assert_eq!(line_limit(), 16 * 1024);
    }
}
