use crate::{
    config::Config,
    layout::{ICON_SIZE, Position, hit_icon, hit_icon_except, snap_axis},
    shm::FrameBuffers,
};
use std::{io, path::PathBuf};
use wayland_client::{
    Connection, Dispatch, EventQueue, QueueHandle, WEnum,
    globals::{GlobalListContents, registry_queue_init},
    protocol::{
        wl_buffer::WlBuffer, wl_compositor::WlCompositor, wl_output::WlOutput,
        wl_pointer::WlPointer, wl_region::WlRegion, wl_seat::WlSeat, wl_shm::WlShm,
        wl_shm_pool::WlShmPool, wl_surface::WlSurface,
    },
};
use wayland_protocols_wlr::layer_shell::v1::client::{
    zwlr_layer_shell_v1::{self, ZwlrLayerShellV1},
    zwlr_layer_surface_v1::{self, ZwlrLayerSurfaceV1},
};

#[derive(Clone, Debug)]
struct DragState {
    index: usize,
    offset_x: f64,
    offset_y: f64,
}

#[derive(Debug)]
pub enum DropAction {
    Reposition { path: PathBuf, position: Position },
    MoveInto { source: PathBuf, destination: PathBuf },
}

#[derive(Default)]
pub struct WaylandState {
    compositor: Option<WlCompositor>,
    shm: Option<WlShm>,
    layer_shell: Option<ZwlrLayerShellV1>,
    seat: Option<WlSeat>,
    pointer: Option<WlPointer>,
    surface: Option<WlSurface>,
    layer_surface: Option<ZwlrLayerSurfaceV1>,
    frame_buffers: Option<FrameBuffers>,
    icons: Vec<Position>,
    paths: Vec<PathBuf>,
    drag: Option<DragState>,
    pointer_x: f64,
    pointer_y: f64,
    pub dirty: bool,
    pub pending_drop: Option<DropAction>,
    pub configured_size: Option<(i32, i32)>,
    pub needs_flush: bool,
    grid_x: i32,
    grid_y: i32,
    offset_x: i32,
    offset_y: i32,
    configured: bool,
}

pub fn sync_wayland_state(state: &mut WaylandState, icons: &[Position], paths: &[PathBuf]) {
    state.drag = None;
    state.pending_drop = None;
    state.icons.clear();
    state.paths.clear();
    state.icons.extend_from_slice(icons);
    state.paths.extend(paths.iter().cloned());
}

fn ensure_framebuffers(
    state: &mut WaylandState,
    qh: &QueueHandle<WaylandState>,
    width: i32,
    height: i32,
) -> io::Result<bool> {
    if let Some(existing) = state.frame_buffers.as_ref()
        && existing.dimensions() == (width.max(1), height.max(1))
    {
        return Ok(true);
    }
    if let Some(existing) = state.frame_buffers.as_ref()
        && existing.any_busy()
    {
        return Ok(false);
    }
    state.frame_buffers = None;
    let shm = state.shm.as_ref().ok_or_else(|| io::Error::other("missing wl_shm"))?;
    state.frame_buffers = Some(FrameBuffers::new(shm, qh, width, height)?);
    Ok(true)
}

pub fn render_frame(state: &mut WaylandState, qh: &QueueHandle<WaylandState>) -> io::Result<()> {
    let (Some(surface), Some((width, height))) = (state.surface.as_ref(), state.configured_size) else {
        return Ok(());
    };
    if !ensure_framebuffers(state, qh, width, height)? {
        state.dirty = true;
        return Ok(());
    }
    let Some(frame_buffers) = state.frame_buffers.as_mut() else {
        return Ok(());
    };
    let Some(buffer_index) = frame_buffers.next_free_index() else {
        state.dirty = true;
        return Ok(());
    };

    frame_buffers.paint(buffer_index, &state.icons);
    let Some(buffer) = frame_buffers.buffer(buffer_index) else {
        state.dirty = true;
        return Ok(());
    };

    surface.attach(Some(buffer), 0, 0);
    surface.damage_buffer(0, 0, width, height);
    surface.commit();
    frame_buffers.mark_busy(buffer_index);

    let region = state
        .compositor
        .as_ref()
        .expect("compositor is bound")
        .create_region(qh, ());
    for position in &state.icons {
        region.add(position.x, position.y, ICON_SIZE, ICON_SIZE);
    }
    surface.set_input_region(Some(&region));
    region.destroy();

    state.needs_flush = true;
    Ok(())
}

pub fn create_layer_surface(
    conn: &Connection,
    icons: Vec<Position>,
    paths: Vec<PathBuf>,
    config: &Config,
) -> io::Result<(EventQueue<WaylandState>, WaylandState)> {
    let (globals, queue) =
        registry_queue_init(conn).map_err(|e| io::Error::other(e.to_string()))?;
    let mut state = WaylandState {
        icons,
        paths,
        grid_x: config.grid_x,
        grid_y: config.grid_y,
        offset_x: config.offset_x,
        offset_y: config.offset_y,
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
    state.seat = globals.bind::<WlSeat, _, _>(&qh, 1..=9, ()).ok();
    let surface = state.compositor.as_ref().unwrap().create_surface(&qh, ());
    let layer = state.layer_shell.as_ref().unwrap().get_layer_surface(
        &surface,
        None,
        zwlr_layer_shell_v1::Layer::Bottom,
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

impl Dispatch<WlSeat, ()> for WaylandState {
    fn event(
        state: &mut Self,
        proxy: &WlSeat,
        event: wayland_client::protocol::wl_seat::Event,
        _: &(),
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        if let wayland_client::protocol::wl_seat::Event::Capabilities { capabilities } = event
            && let WEnum::Value(capabilities) = capabilities
            && capabilities.contains(wayland_client::protocol::wl_seat::Capability::Pointer)
            && state.pointer.is_none()
        {
            state.pointer = Some(proxy.get_pointer(qh, ()));
        }
    }
}

impl Dispatch<WlPointer, ()> for WaylandState {
    fn event(
        state: &mut Self,
        _: &WlPointer,
        event: wayland_client::protocol::wl_pointer::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        use wayland_client::protocol::wl_pointer::{ButtonState, Event};
        match event {
            Event::Enter {
                surface_x,
                surface_y,
                ..
            }
            | Event::Motion {
                surface_x,
                surface_y,
                ..
            } => {
                state.pointer_x = surface_x;
                state.pointer_y = surface_y;
                if let Some(drag) = &state.drag {
                    let position = Position {
                        x: snap_axis(surface_x - drag.offset_x, state.offset_x, state.grid_x),
                        y: snap_axis(surface_y - drag.offset_y, state.offset_y, state.grid_y),
                    };
                    if state.icons[drag.index] != position {
                        state.icons[drag.index] = position;
                        state.dirty = true;
                    }
                }
            }
            Event::Button {
                button: 0x110,
                state: button_state,
                ..
            } => {
                if button_state == WEnum::Value(ButtonState::Pressed) && state.drag.is_none() {
                    if let Some(index) = hit_icon(&state.icons, state.pointer_x, state.pointer_y) {
                        let icon = state.icons[index];
                        state.drag = Some(DragState {
                            index,
                            offset_x: state.pointer_x - f64::from(icon.x),
                            offset_y: state.pointer_y - f64::from(icon.y),
                        });
                    }
                } else if button_state == WEnum::Value(ButtonState::Released)
                    && let Some(drag) = state.drag.take()
                {
                    let source = state.paths[drag.index].clone();
                    let position = state.icons[drag.index];
                    let target = hit_icon_except(&state.icons, state.pointer_x, state.pointer_y, drag.index);
                    state.pending_drop = target
                        .filter(|&index| state.paths[index].is_dir())
                        .and_then(|index| {
                            source.file_name().map(|name| DropAction::MoveInto {
                                destination: state.paths[index].join(name),
                                source: source.clone(),
                            })
                        })
                        .or(Some(DropAction::Reposition {
                            path: source,
                            position,
                        }));
                    state.dirty = true;
                }
            }
            _ => {}
        }
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

impl Dispatch<WlBuffer, usize> for WaylandState {
    fn event(
        state: &mut Self,
        _: &WlBuffer,
        event: wayland_client::protocol::wl_buffer::Event,
        index: &usize,
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        if matches!(event, wayland_client::protocol::wl_buffer::Event::Release) {
            if let Some(frame_buffers) = state.frame_buffers.as_mut() {
                frame_buffers.mark_released(*index);
            }
            if state.dirty {
                state.dirty = false;
                if let Err(error) = render_frame(state, qh) {
                    eprintln!("ldicons: unable to redraw Wayland buffer: {error}");
                }
            }
        }
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
            state.configured_size = Some((width as i32, height as i32));
            state.dirty = true;
            if let Err(error) = render_frame(state, qh) {
                eprintln!("ldicons: unable to render Wayland buffer: {error}");
            }
            state.dirty = false;
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
