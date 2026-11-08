use crate::{
    config::{Args, Config},
    layout::Position,
    nsd::NsdClient,
    positions::{load_positions, save_positions},
    watch::LinuxWatch,
    wayland::{DropAction, create_layer_surface, render_frame, sync_wayland_state},
};
use libc::{EPOLL_CLOEXEC, EPOLL_CTL_ADD, EPOLL_CTL_DEL, EPOLLIN, epoll_create1, epoll_ctl, epoll_event, epoll_wait};
use std::{
    fs, io,
    os::fd::{AsRawFd, RawFd},
    path::PathBuf,
};
use wayland_client::Connection;

#[derive(Debug)]
pub struct Icon {
    pub path: PathBuf,
    pub position: Position,
}

pub struct App {
    config: Config,
    positions_path: PathBuf,
    icons: Vec<Icon>,
    watch: LinuxWatch,
    nsd: Option<NsdClient>,
}

impl App {
    pub fn new(args: &Args) -> io::Result<Self> {
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

    fn icon_state(&self) -> (Vec<Position>, Vec<PathBuf>) {
        (
            self.icons.iter().map(|icon| icon.position).collect(),
            self.icons.iter().map(|icon| icon.path.clone()).collect(),
        )
    }
}

fn scan_desktop(
    config: &Config,
    positions: &std::collections::HashMap<String, Position>,
) -> io::Result<Vec<Icon>> {
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
            let position = positions.get(&key).copied().unwrap_or_else(|| Position {
                x: config.offset_x + (index as i32 / 6) * config.grid_x,
                y: config.offset_y + (index as i32 % 6) * config.grid_y,
            });
            Icon { path, position }
        })
        .collect())
}

fn apply_drop(app: &mut App, action: DropAction) -> io::Result<()> {
    match action {
        DropAction::MoveInto {
            source,
            destination,
        } => {
            if source == destination {
                return Ok(());
            }
            if destination.exists() {
                eprintln!(
                    "ldicons: refusing to replace existing desktop item {}",
                    destination.display()
                );
                return Ok(());
            }
            if let Err(error) = fs::rename(&source, &destination) {
                eprintln!(
                    "ldicons: unable to move {} into {}: {error}",
                    source.display(),
                    destination.display()
                );
                return Ok(());
            }
            app.refresh()?;
        }
        DropAction::Reposition { path, position } => {
            if let Some(icon) = app.icons.iter_mut().find(|icon| icon.path == path) {
                icon.position = position;
            }
        }
    }
    save_positions(&app.positions_path, &app.icons)
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

fn epoll_delete(epoll: RawFd, fd: RawFd) -> io::Result<()> {
    if unsafe { epoll_ctl(epoll, EPOLL_CTL_DEL, fd, std::ptr::null_mut()) } < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

pub fn run(mut app: App, args: &Args) -> io::Result<()> {
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
    let (positions, paths) = app.icon_state();
    let (mut queue, mut state) = create_layer_surface(&conn, positions, paths, &app.config)?;

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
                let (positions, paths) = app.icon_state();
                sync_wayland_state(&mut state, &positions, &paths);
                state.dirty = true;
            }

            let mut nsd_disconnected = false;
            if let Some(nsd) = &mut app.nsd
                && fd == nsd.fd()
                && !nsd.read_events(args.verbose)
            {
                let nsd_fd = nsd.fd();
                epoll_delete(epoll, nsd_fd)?;
                nsd_disconnected = true;
            }
            if nsd_disconnected {
                app.nsd = None;
            }

            if fd == wayland_fd {
                queue
                    .blocking_dispatch(&mut state)
                    .map_err(|e| io::Error::other(e.to_string()))?;
            }

            if let Some(action) = state.pending_drop.take() {
                apply_drop(&mut app, action)?;
                let (positions, paths) = app.icon_state();
                sync_wayland_state(&mut state, &positions, &paths);
                state.dirty = true;
            }
        }

        if state.dirty && state.configured_size.is_some() {
            state.dirty = false;
            render_frame(&mut state, &queue.handle())?;
        }

        if state.needs_flush {
            conn.flush().map_err(|error| io::Error::other(error.to_string()))?;
            state.needs_flush = false;
        }
    }
}
