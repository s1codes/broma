// Copyright (c) 2026 Sayantan Banerjee
// Licensed under the BSD 3-Clause License.
// See LICENSE for details.

//! Linux-specific audio routing and independent Virtual Microphone management.
//!
//! Creates and manages Broma's standalone virtual microphone inside PipeWire
//! (`pipewire-pulse`) or PulseAudio using pure-Rust socket IPC (`pulseaudio::protocol`),
//! requiring zero external CLI binaries (`pactl`) and zero C build dependencies.
//!
//! Audio graph when `"Broma Virtual Mic (Auto)"` is active:
//! 1. `module-null-sink` (`broma_soundboard_sink`) — hidden mixing bus fed by Broma Stream 2.
//! 2. `module-remap-source` (`broma_virtual_mic`) — exposes `"Broma Virtual Microphone"`
//!    as a true capture device so Discord/Chromium/Steam list it as a real microphone.
//! 3. `module-loopback` — routes the user's selected hardware microphone straight
//!    into `broma_soundboard_sink` inside PipeWire/PulseAudio at 0% idle Broma CPU.

use std::ffi::CString;
use std::io::BufReader;
use std::os::unix::net::UnixStream;
use std::str::FromStr;
use std::time::Duration;

use cpal::traits::{DeviceTrait, HostTrait};
use cpal::{Device, DeviceId, Host};
use pulseaudio::protocol::{
    self, AuthParams, AuthReply, Command, CommandReply, LoadModuleParams, LookupReply,
    ModuleInfoList, Prop, Props, ServerInfo, SetClientNameReply, SourceInfo,
};

use crate::audio::mic_bridge::{MicBridge, MicPassthroughConsumer};
use crate::audio::platform::ActiveVirtualRoute;
use crate::audio::{DEVICE_DISABLED, DEVICE_SYSTEM_DEFAULT_MIC, device_display_name};

/// User-facing option in the Output dropdown that auto-provisions Broma's virtual mic.
pub const DEVICE_BROMA_VIRTUAL_MIC: &str = "Broma Virtual Mic (Auto)";

const BROMA_SINK_NAME: &str = "broma_soundboard_sink";
const BROMA_SINK_DESC: &str = "Broma_Soundboard_Bus";
pub(crate) const BROMA_MIC_NAME: &str = "broma_virtual_mic";
pub(crate) const BROMA_MIC_DESC: &str = "Broma Virtual Microphone";

const SOCKET_IO_TIMEOUT: Duration = Duration::from_secs(2);

pub struct LinuxAudioRouter {
    /// Server module indices loaded by this Broma instance (unloaded in reverse order).
    loaded_modules: Vec<u32>,
}

impl Default for LinuxAudioRouter {
    fn default() -> Self {
        Self::new()
    }
}

impl LinuxAudioRouter {
    pub fn new() -> Self {
        // Clean up any orphaned Broma virtual mic modules left behind by a prior SIGKILL/crash.
        let _ = sweep_stale_broma_modules();
        Self {
            loaded_modules: Vec::new(),
        }
    }

    pub fn list_microphones(&self, host: &Host) -> Vec<String> {
        let mut out = Vec::new();
        out.push(DEVICE_SYSTEM_DEFAULT_MIC.to_string());

        if let Ok(mut conn) = PulseControlConn::connect()
            && let Ok(sources) = conn.list_sources()
        {
            for src in sources {
                if is_selectable_linux_mic(&src)
                    && let Some(desc_c) = &src.description
                    && let Ok(desc) = desc_c.to_str()
                {
                    let trimmed = desc.trim();
                    if !trimmed.is_empty()
                        && trimmed != DEVICE_SYSTEM_DEFAULT_MIC
                        && trimmed != DEVICE_DISABLED
                        && !out.iter().any(|existing| existing == trimmed)
                    {
                        out.push(trimmed.to_string());
                    }
                }
            }
            out.push(DEVICE_DISABLED.to_string());
            return out;
        }

        // Fallback to cpal input enumeration if Pulse/PipeWire socket is unavailable (pure ALSA).
        if let Ok(devices) = host.input_devices() {
            for dev in devices {
                let name = device_display_name(&dev);
                if !name.is_empty()
                    && !is_internal_or_monitor_name(&name)
                    && name != DEVICE_SYSTEM_DEFAULT_MIC
                    && name != DEVICE_DISABLED
                    && !out.contains(&name)
                {
                    out.push(name);
                }
            }
        }

        out.push(DEVICE_DISABLED.to_string());
        out
    }

    pub fn list_virtual_outputs(&self, host: &Host) -> Vec<String> {
        let mut out = Vec::new();

        if pulseaudio::socket_path_from_env().is_some() {
            out.push(DEVICE_BROMA_VIRTUAL_MIC.to_string());
        }

        if let Ok(devices) = host.output_devices() {
            for dev in devices {
                let name = device_display_name(&dev);
                if !name.is_empty()
                    && !is_broma_internal_sink_name(&name)
                    && name != DEVICE_BROMA_VIRTUAL_MIC
                    && name != DEVICE_DISABLED
                    && !out.contains(&name)
                {
                    out.push(name);
                }
            }
        }

        out.push(DEVICE_DISABLED.to_string());
        out
    }

    pub fn default_virtual_output(&self, available: &[String]) -> String {
        if available.iter().any(|d| d == DEVICE_BROMA_VIRTUAL_MIC) {
            DEVICE_BROMA_VIRTUAL_MIC.to_string()
        } else {
            DEVICE_DISABLED.to_string()
        }
    }

    pub fn prepare_route(
        &mut self,
        host: &Host,
        mic_selection: &str,
        output_selection: &str,
    ) -> Result<Option<ActiveVirtualRoute>, String> {
        self.teardown();

        if output_selection.is_empty() || output_selection == DEVICE_DISABLED {
            return Ok(None);
        }

        if output_selection == DEVICE_BROMA_VIRTUAL_MIC {
            return self
                .provision_managed_virtual_mic(host, mic_selection)
                .map(Some);
        }

        // User explicitly picked a pre-existing external Linux sink.
        let sink_device = find_linux_output_device(host, output_selection)
            .ok_or_else(|| format!("Linux output sink '{output_selection}' is not available"))?;

        let (mic_bridge, route_warning) = self.open_mic_bridge(host, mic_selection);

        Ok(Some(ActiveVirtualRoute {
            sink_device,
            mic_bridge,
            route_warning,
        }))
    }

    /// Opens in-process `cpal` mic capture for non-managed (external) sinks.
    ///
    /// The managed `"Broma Virtual Mic (Auto)"` route keeps using
    /// `module-loopback` inside PipeWire/PulseAudio and returns `None`.
    pub fn open_mic_bridge(
        &self,
        host: &Host,
        mic_selection: &str,
    ) -> (Option<(MicBridge, MicPassthroughConsumer)>, Option<String>) {
        if mic_selection.is_empty() || mic_selection == DEVICE_DISABLED {
            return (None, None);
        }

        let Some(mic_dev) = find_linux_input_device(host, mic_selection) else {
            return (
                None,
                Some(format!(
                    "Microphone '{mic_selection}' not found — soundboard active without mic passthrough"
                )),
            );
        };

        match MicBridge::open(mic_dev) {
            Ok(bridge) => (Some(bridge), None),
            Err(err) => (
                None,
                Some(format!("Microphone passthrough unavailable: {err}")),
            ),
        }
    }

    pub fn teardown(&mut self) {
        if self.loaded_modules.is_empty() {
            return;
        }
        if let Ok(mut conn) = PulseControlConn::connect() {
            while let Some(module_id) = self.loaded_modules.pop() {
                let _ = conn.unload_module(module_id);
            }
        } else {
            self.loaded_modules.clear();
        }
    }

    fn provision_managed_virtual_mic(
        &mut self,
        host: &Host,
        mic_selection: &str,
    ) -> Result<ActiveVirtualRoute, String> {
        let _ = sweep_stale_broma_modules();

        let mut conn = PulseControlConn::connect()
            .map_err(|e| format!("Could not connect to PipeWire/PulseAudio: {e}"))?;

        // 1. Create hidden mixing bus sink for Broma Stream 2 + Mic Loopback.
        let sink_args = format!(
            "sink_name={BROMA_SINK_NAME} sink_properties=device.description={BROMA_SINK_DESC} rate=48000 channels=2"
        );
        let sink_mod_id = conn
            .load_module("module-null-sink", &sink_args)
            .map_err(|e| format!("Failed to create virtual sink: {e}"))?;
        self.loaded_modules.push(sink_mod_id);

        // 2. Remap the sink's `.monitor` into a real Source so Discord/Chromium/games list it.
        let remap_args = format!(
            "source_name={BROMA_MIC_NAME} master={BROMA_SINK_NAME}.monitor source_properties='device.description=\"{BROMA_MIC_DESC}\"'"
        );
        match conn.load_module("module-remap-source", &remap_args) {
            Ok(remap_mod_id) => {
                self.loaded_modules.push(remap_mod_id);
            }
            Err(err) => {
                self.teardown();
                return Err(format!("Failed to create virtual microphone source: {err}"));
            }
        }

        // 3. Route the user's selected hardware microphone straight through to the virtual sink
        //    inside PipeWire/PulseAudio (0% idle CPU in Broma, untouched voice when idle).
        let mut route_warning = None;
        if !mic_selection.is_empty() && mic_selection != DEVICE_DISABLED {
            match resolve_pulse_mic_source_name(&mut conn, mic_selection) {
                Some(source_name) => {
                    let loopback_args = format!(
                        "source={source_name} sink={BROMA_SINK_NAME} latency_msec=20 source_dont_move=true sink_dont_move=true"
                    );
                    match conn.load_module("module-loopback", &loopback_args) {
                        Ok(loop_mod_id) => self.loaded_modules.push(loop_mod_id),
                        Err(err) => {
                            route_warning =
                                Some(format!("Microphone loopback failed to load: {err}"));
                        }
                    }
                }
                None => {
                    route_warning = Some(format!(
                        "Microphone '{mic_selection}' not found — virtual mic active without voice passthrough"
                    ));
                }
            }
        }

        drop(conn);

        // Locate the newly created sink via `cpal` so Stream 2 opens it directly.
        let sink_device = find_broma_bus_device(host).ok_or_else(|| {
            self.teardown();
            "Virtual sink was created but could not be opened by cpal".to_string()
        })?;

        Ok(ActiveVirtualRoute {
            sink_device,
            mic_bridge: None,
            route_warning,
        })
    }
}

impl Drop for LinuxAudioRouter {
    fn drop(&mut self) {
        self.teardown();
    }
}

/// Returns true if a sink name belongs to Broma's hidden internal mixing bus.
pub fn is_broma_internal_sink_name(name: &str) -> bool {
    name == BROMA_SINK_DESC
        || name == BROMA_SINK_NAME
        || name.contains(BROMA_SINK_NAME)
        || name.contains(BROMA_SINK_DESC)
}

fn is_selectable_linux_mic(src: &SourceInfo) -> bool {
    // Exclude `.monitor` loopback sources and Broma's own virtual mic to prevent feedback loops.
    if src.monitor_of_sink_index.is_some() {
        return false;
    }
    let raw_name = src.name.to_string_lossy();
    if raw_name == BROMA_MIC_NAME || raw_name.ends_with(".monitor") {
        return false;
    }
    if let Some(desc) = &src.description {
        let d = desc.to_string_lossy();
        if d == BROMA_MIC_DESC || is_internal_or_monitor_name(&d) {
            return false;
        }
    }
    true
}

pub(crate) fn is_internal_or_monitor_name(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    lower.starts_with("monitor of ")
        || lower.contains("broma virtual")
        || lower.contains("broma_soundboard_bus")
        || lower.contains("discard all samples")
}

pub(crate) fn is_stale_broma_module_arg(argument: &str) -> bool {
    argument.contains(BROMA_SINK_NAME) || argument.contains(BROMA_MIC_NAME)
}

/// Unloads any leftover PipeWire/PulseAudio modules referencing Broma's virtual sink/mic.
fn sweep_stale_broma_modules() -> Result<(), String> {
    let mut conn = PulseControlConn::connect()?;
    let modules = conn.list_modules()?;

    let mut stale_ids: Vec<u32> = modules
        .into_iter()
        .filter(|m| {
            m.argument
                .as_ref()
                .is_some_and(|arg| is_stale_broma_module_arg(&arg.to_string_lossy()))
        })
        .map(|m| m.index)
        .collect();

    // Unload dependents (loopback, remap-source) before the underlying null-sink.
    stale_ids.sort_unstable();
    for id in stale_ids.into_iter().rev() {
        let _ = conn.unload_module(id);
    }
    Ok(())
}

fn resolve_pulse_mic_source_name(conn: &mut PulseControlConn, selection: &str) -> Option<String> {
    let sources = conn.list_sources().ok()?;

    if selection == DEVICE_SYSTEM_DEFAULT_MIC {
        if let Ok(info) = conn.server_info()
            && let Some(default_src) = info.default_source_name
        {
            let default_str = default_src.to_string_lossy();
            if let Some(matched) = sources
                .iter()
                .find(|s| s.name.to_string_lossy() == default_str && is_selectable_linux_mic(s))
            {
                return Some(matched.name.to_string_lossy().into_owned());
            }
        }
        // If OS default source was somehow pointing at a monitor or Broma mic, pick first real mic.
        return sources
            .iter()
            .find(|s| is_selectable_linux_mic(s))
            .map(|s| s.name.to_string_lossy().into_owned());
    }

    sources
        .iter()
        .find(|s| {
            is_selectable_linux_mic(s)
                && s.description
                    .as_ref()
                    .is_some_and(|d| d.to_string_lossy().trim() == selection)
        })
        .map(|s| s.name.to_string_lossy().into_owned())
}

fn find_broma_bus_device(host: &Host) -> Option<Device> {
    if let Ok(id) = DeviceId::from_str(&format!("pulseaudio:{BROMA_SINK_NAME}"))
        && let Some(dev) = host.device_by_id(&id)
    {
        return Some(dev);
    }

    let mut devices = host.output_devices().ok()?;
    devices.find(|dev| {
        let desc = device_display_name(dev);
        desc == BROMA_SINK_DESC
            || desc == BROMA_SINK_NAME
            || dev.id().ok().is_some_and(|id| id.id() == BROMA_SINK_NAME)
    })
}

fn find_linux_output_device(host: &Host, selection: &str) -> Option<Device> {
    let mut devices = host.output_devices().ok()?;
    devices.find(|dev| device_display_name(dev) == selection)
}

fn find_linux_input_device(host: &Host, selection: &str) -> Option<Device> {
    if selection == DEVICE_SYSTEM_DEFAULT_MIC {
        return host.default_input_device();
    }
    let mut devices = host.input_devices().ok()?;
    devices.find(|dev| device_display_name(dev) == selection)
}

/// Short-lived synchronous control connection to PulseAudio / `pipewire-pulse`.
pub(crate) struct PulseControlConn {
    reader: BufReader<UnixStream>,
    protocol_version: u16,
    next_seq: u32,
}

impl PulseControlConn {
    pub(crate) fn connect() -> Result<Self, String> {
        let socket_path = pulseaudio::socket_path_from_env()
            .ok_or_else(|| "PulseAudio / PipeWire-Pulse socket not found".to_string())?;
        let cookie = pulseaudio::cookie_path_from_env()
            .and_then(|p| std::fs::read(p).ok())
            .unwrap_or_default();

        let stream = UnixStream::connect(&socket_path)
            .map_err(|e| format!("Socket connect failed ({}): {e}", socket_path.display()))?;
        let _ = stream.set_read_timeout(Some(SOCKET_IO_TIMEOUT));
        let _ = stream.set_write_timeout(Some(SOCKET_IO_TIMEOUT));

        let mut conn = Self {
            reader: BufReader::new(stream),
            protocol_version: protocol::MAX_VERSION,
            next_seq: 0,
        };

        let auth = AuthParams {
            version: protocol::MAX_VERSION,
            supports_shm: false,
            supports_memfd: false,
            cookie,
        };
        let auth_reply: AuthReply = conn.roundtrip_reply(Command::Auth(auth))?;
        conn.protocol_version = protocol::MAX_VERSION.min(auth_reply.version);

        let mut props = Props::new();
        props.set(Prop::ApplicationName, c"Broma");
        if let Ok(pid) = CString::new(std::process::id().to_string()) {
            props.set(Prop::ApplicationProcessId, pid);
        }
        let _: SetClientNameReply = conn.roundtrip_reply(Command::SetClientName(props))?;

        Ok(conn)
    }

    fn next_seq(&mut self) -> u32 {
        let seq = self.next_seq;
        self.next_seq = self.next_seq.wrapping_add(1);
        seq
    }

    fn roundtrip_reply<R: CommandReply>(&mut self, cmd: Command) -> Result<R, String> {
        let seq = self.next_seq();
        protocol::write_command_message(self.reader.get_mut(), seq, &cmd, self.protocol_version)
            .map_err(|e| format!("Pulse write error: {e}"))?;
        let (reply_seq, reply) =
            protocol::read_reply_message::<R>(&mut self.reader, self.protocol_version)
                .map_err(|e| format!("Pulse reply error: {e}"))?;
        if reply_seq != seq {
            return Err("Out-of-order PulseAudio reply sequence".to_string());
        }
        Ok(reply)
    }

    fn roundtrip_ack(&mut self, cmd: Command) -> Result<(), String> {
        let seq = self.next_seq();
        protocol::write_command_message(self.reader.get_mut(), seq, &cmd, self.protocol_version)
            .map_err(|e| format!("Pulse write error: {e}"))?;
        let reply_seq = protocol::read_ack_message(&mut self.reader)
            .map_err(|e| format!("Pulse ack error: {e}"))?;
        if reply_seq != seq {
            return Err("Out-of-order PulseAudio ack sequence".to_string());
        }
        Ok(())
    }

    fn load_module(&mut self, name: &str, args: &str) -> Result<u32, String> {
        let name_c = CString::new(name).map_err(|_| "Invalid module name".to_string())?;
        let args_c = CString::new(args).map_err(|_| "Invalid module arguments".to_string())?;
        let reply: LookupReply = self.roundtrip_reply(Command::LoadModule(LoadModuleParams {
            name: name_c,
            arguments: Some(args_c),
        }))?;
        Ok(reply.0)
    }

    fn unload_module(&mut self, index: u32) -> Result<(), String> {
        self.roundtrip_ack(Command::UnloadModule(index))
    }

    fn list_modules(&mut self) -> Result<ModuleInfoList, String> {
        self.roundtrip_reply(Command::GetModuleInfoList)
    }

    pub(crate) fn list_sources(&mut self) -> Result<Vec<SourceInfo>, String> {
        self.roundtrip_reply(Command::GetSourceInfoList)
    }

    fn server_info(&mut self) -> Result<ServerInfo, String> {
        self.roundtrip_reply(Command::GetServerInfo)
    }
}
