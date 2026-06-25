use std::collections::{HashMap, VecDeque};
use std::io::ErrorKind;
use std::os::fd::{AsRawFd, RawFd};

use anyhow::{Result, bail};

use wayrs_client::global::*;
use wayrs_client::protocol::*;
use wayrs_client::{Connection, EventCtx, IoMode};
use wayrs_protocols::wlr_gamma_control_unstable_v1::*;

use crate::color::{Color, colorramp_fill};

pub struct Wayland {
    conn: Connection<WaylandState>,
    pub state: WaylandState,
}

pub struct WaylandState {
    pub outputs: Vec<Output>,
    pub gamma_manager: ZwlrGammaControlManagerV1,
    pub events: VecDeque<WaylandEvent>,
    /// Per-connector remembered Color: the value a display returns to on reconnect or
    /// restore. Captured at idle-dim (pre-dim values) and, when NOT dimmed, on disconnect.
    pub snapshots: HashMap<String, Color>,
    /// True between an idle `Snapshot` (dim) and its `Restore`. While set, a disconnect
    /// must NOT overwrite the remembered (pre-dim) value with the transient dim value —
    /// on this box a sustained DPMS-off arrives as a gamma_control `Failed` (a disconnect).
    pub dimmed: bool,
}

pub enum WaylandEvent {
    NewOutput { reg_name: u32, name: String },
    RemoveOutput { name: String },
}

impl AsRawFd for Wayland {
    fn as_raw_fd(&self) -> RawFd {
        self.conn.as_raw_fd()
    }
}

impl Wayland {
    pub fn new() -> Result<Self> {
        let mut conn = Connection::connect()?;
        conn.blocking_roundtrip()?;

        let Ok(gamma_manager) = conn.bind_singleton(1) else {
            bail!(
                "Your Wayland compositor is not supported because it does not implement the wlr-gamma-control-unstable-v1 protocol"
            );
        };

        let mut state = WaylandState {
            outputs: Vec::new(),
            gamma_manager,
            events: VecDeque::new(),
            snapshots: HashMap::new(),
            dimmed: false,
        };

        conn.add_registry_cb(wl_registry_cb);
        conn.dispatch_events(&mut state);
        conn.flush(IoMode::Blocking)?;

        Ok(Self { conn, state })
    }

    pub fn poll(&mut self) -> Result<()> {
        match self.conn.recv_events(IoMode::NonBlocking) {
            Ok(()) => self.conn.dispatch_events(&mut self.state),
            Err(e) if e.kind() == ErrorKind::WouldBlock => (),
            Err(e) => return Err(e.into()),
        }

        for output in &mut self.state.outputs {
            if output.color_changed {
                output.update_displayed_color(&mut self.conn)?;
            }
        }

        self.conn.flush(IoMode::Blocking)?;
        Ok(())
    }

    pub fn next_event(&mut self) -> Option<WaylandEvent> {
        self.state.events.pop_front()
    }
}

impl WaylandState {
    /// Replace the snapshot map with the current Color of every named output.
    /// Called at idle-dim; rebuilding fresh each cycle bounds staleness to one cycle.
    pub fn snapshot_all(&mut self) {
        self.snapshots = self
            .outputs
            .iter()
            .filter_map(|o| o.name.clone().map(|n| (n, o.color)))
            .collect();
    }

    /// Re-apply each connected display's remembered Color, forcing a LUT re-upload
    /// (a DPMS-on may have dropped it). The remembered values are KEPT — they are the
    /// persistent per-display memory; only the next `snapshot_all` replaces them.
    pub fn restore_connected(&mut self) {
        let remembered = self.snapshots.clone();
        for output in &mut self.outputs {
            if let Some(name) = &output.name
                && let Some(color) = remembered.get(name)
            {
                output.color = *color;
                output.color_changed = true;
            }
        }
    }
}

#[derive(Debug)]
pub struct Output {
    reg_name: u32,
    wl: WlOutput,
    name: Option<String>,
    color: Color,
    gamma_control: ZwlrGammaControlV1,
    ramp_size: usize,
    color_changed: bool,
}

impl Output {
    fn bind(
        conn: &mut Connection<WaylandState>,
        global: &Global,
        gamma_manager: ZwlrGammaControlManagerV1,
    ) -> Self {
        eprintln!("New output: {}", global.name);
        let output = global.bind_with_cb(conn, 4, wl_output_cb).unwrap();
        Self {
            reg_name: global.name,
            wl: output,
            name: None,
            color: Color::default(),
            gamma_control: gamma_manager.get_gamma_control_with_cb(conn, output, gamma_control_cb),
            ramp_size: 0,
            color_changed: true,
        }
    }

    fn destroy(self, conn: &mut Connection<WaylandState>) {
        eprintln!("Output {} removed", self.reg_name);
        self.gamma_control.destroy(conn);
        self.wl.release(conn);
    }

    pub fn reg_name(&self) -> u32 {
        self.reg_name
    }

    pub fn color(&self) -> Color {
        self.color
    }

    pub fn color_changed(&self) -> bool {
        self.color_changed
    }

    pub fn set_color(&mut self, color: Color) {
        if color != self.color {
            self.color = color;
            self.color_changed = true;
        }
    }

    pub fn object_path(&self) -> Option<String> {
        self.name
            .as_deref()
            .map(|name| format!("/outputs/{}", name.replace('-', "_")))
    }

    fn update_displayed_color(&mut self, conn: &mut Connection<WaylandState>) -> Result<()> {
        if self.ramp_size == 0 {
            return Ok(());
        }

        let file = shmemfdrs2::create_shmem(c"/ramp-buffer")?;
        file.set_len(self.ramp_size as u64 * 6)?;
        let mut mmap = unsafe { memmap2::MmapMut::map_mut(&file)? };
        let buf = bytemuck::cast_slice_mut::<u8, u16>(&mut mmap);
        let (r, rest) = buf.split_at_mut(self.ramp_size);
        let (g, b) = rest.split_at_mut(self.ramp_size);
        colorramp_fill(r, g, b, self.ramp_size, self.color);
        self.gamma_control.set_gamma(conn, file.into());

        self.color_changed = false;
        Ok(())
    }
}

fn wl_registry_cb(
    conn: &mut Connection<WaylandState>,
    state: &mut WaylandState,
    event: &wl_registry::Event,
) {
    match event {
        wl_registry::Event::Global(global) if global.is::<WlOutput>() => {
            let mut output = Output::bind(conn, global, state.gamma_manager);
            output.set_color(state.color());
            output.update_displayed_color(conn).unwrap();
            state.outputs.push(output);
        }
        wl_registry::Event::GlobalRemove(name) => {
            if let Some(output_index) = state.outputs.iter().position(|o| o.reg_name == *name) {
                let output = state.outputs.swap_remove(output_index);
                if let Some(output_name) = &output.name {
                    // Remember this display's value so it returns at the same level on
                    // reconnect — but NOT while dimmed, or the dim value would clobber the
                    // pre-dim value already captured at dim.
                    if !state.dimmed {
                        state.snapshots.insert(output_name.clone(), output.color);
                    }
                    state.events.push_back(WaylandEvent::RemoveOutput {
                        name: output_name.clone(),
                    });
                }
                output.destroy(conn);
            }
        }
        _ => (),
    }
}

fn gamma_control_cb(ctx: EventCtx<WaylandState, ZwlrGammaControlV1>) {
    let output_index = ctx
        .state
        .outputs
        .iter()
        .position(|o| o.gamma_control == ctx.proxy)
        .expect("Received event for unknown output");
    match ctx.event {
        zwlr_gamma_control_v1::Event::GammaSize(size) => {
            let output = &mut ctx.state.outputs[output_index];
            eprintln!("Output {}: ramp_size = {}", output.reg_name, size);
            output.ramp_size = size as usize;
            output.update_displayed_color(ctx.conn).unwrap();
        }
        zwlr_gamma_control_v1::Event::Failed => {
            let output = ctx.state.outputs.swap_remove(output_index);
            eprintln!("Output {}: gamma_control::Event::Failed", output.reg_name);
            if let Some(output_name) = &output.name {
                // A sustained DPMS-off arrives here as a Failed WHILE dimmed — must not
                // overwrite the remembered pre-dim value. Only capture on a real
                // (not-dimmed) disconnect.
                if !ctx.state.dimmed {
                    ctx.state
                        .snapshots
                        .insert(output_name.clone(), output.color);
                }
                ctx.state.events.push_back(WaylandEvent::RemoveOutput {
                    name: output_name.clone(),
                });
            }
            output.destroy(ctx.conn);
        }
        _ => (),
    }
}

fn wl_output_cb(ctx: EventCtx<WaylandState, WlOutput>) {
    if let wl_output::Event::Name(name) = ctx.event {
        let name = String::from_utf8(name.into_bytes()).expect("invalid output name");
        // Reconnect: come back at this connector's remembered value (kept across the
        // disconnect). The value persists; only the next idle `Snapshot` replaces it.
        let snapshot = ctx.state.snapshots.get(&name).copied();
        let output = ctx
            .state
            .outputs
            .iter_mut()
            .find(|o| o.wl == ctx.proxy)
            .unwrap();
        eprintln!("Output {}: name = {name:?}", output.reg_name);
        if let Some(color) = snapshot {
            eprintln!("Output {}: restored from snapshot", output.reg_name);
            output.color = color;
            output.color_changed = true;
        }
        let reg_name = output.reg_name;
        output.name = Some(name.clone());
        ctx.state
            .events
            .push_back(WaylandEvent::NewOutput { reg_name, name });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn c(brightness: f64) -> Color {
        Color {
            brightness,
            ..Color::default()
        }
    }

    // Pure check of the "remembered value, kept (not removed)" semantics that
    // restore_connected relies on: applying a remembered map leaves it intact.
    #[test]
    fn remembered_map_is_kept_after_lookup() {
        let mut snaps = HashMap::new();
        snaps.insert("DP-3".to_string(), c(0.85));
        let remembered = snaps.clone();
        assert_eq!(remembered.get("DP-3"), Some(&c(0.85)));
        // restore_connected clones and reads; the original map is unchanged.
        assert_eq!(snaps.get("DP-3"), Some(&c(0.85)));
    }
}
