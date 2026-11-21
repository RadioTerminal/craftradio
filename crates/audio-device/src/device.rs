use cpal::traits::{DeviceTrait, HostTrait};
use std::panic::{AssertUnwindSafe, catch_unwind};

use crate::DeviceError;

/// A device as listed to the user.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceInfo {
    /// Name to pass to `Output::open` / `Input::open`.
    pub name: String,
    /// Whether this is the system default for its direction.
    pub is_default: bool,
    /// Common sample rates the device supports, ascending.
    pub sample_rates: Vec<u32>,
    /// Channel count of the device's default configuration.
    pub channels: u16,
}

const COMMON_RATES: [u32; 8] = [
    8_000, 22_050, 32_000, 44_100, 48_000, 88_200, 96_000, 192_000,
];

/// The first available audio host, or `None` (never panics).
pub(crate) fn host() -> Option<cpal::Host> {
    catch_unwind(|| {
        cpal::available_hosts()
            .into_iter()
            .find_map(|id| cpal::host_from_id(id).ok())
    })
    .ok()
    .flatten()
}

pub(crate) fn device_name(device: &cpal::Device) -> String {
    match device.description() {
        Ok(d) => d.name().to_string(),
        Err(_) => device.id().map(|i| i.to_string()).unwrap_or_default(),
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Direction {
    Output,
    Input,
}

fn devices(host: &cpal::Host, dir: Direction) -> Vec<cpal::Device> {
    let found = match dir {
        Direction::Output => host.output_devices().map(Iterator::collect),
        Direction::Input => host.input_devices().map(Iterator::collect),
    };
    found.unwrap_or_default()
}

fn default_device(host: &cpal::Host, dir: Direction) -> Option<cpal::Device> {
    match dir {
        Direction::Output => host.default_output_device(),
        Direction::Input => host.default_input_device(),
    }
}

fn ranges(device: &cpal::Device, dir: Direction) -> Vec<(u32, u32)> {
    let collect = |it: &mut dyn Iterator<Item = cpal::SupportedStreamConfigRange>| {
        it.map(|r| (r.min_sample_rate(), r.max_sample_rate()))
            .collect::<Vec<_>>()
    };
    match dir {
        Direction::Output => device
            .supported_output_configs()
            .map(|mut i| collect(&mut i))
            .unwrap_or_default(),
        Direction::Input => device
            .supported_input_configs()
            .map(|mut i| collect(&mut i))
            .unwrap_or_default(),
    }
}

fn default_config(
    device: &cpal::Device,
    dir: Direction,
) -> Result<cpal::SupportedStreamConfig, DeviceError> {
    match dir {
        Direction::Output => device.default_output_config(),
        Direction::Input => device.default_input_config(),
    }
    .map_err(|e| DeviceError::Unsupported(e.to_string()))
}

fn list(dir: Direction) -> Vec<DeviceInfo> {
    catch_unwind(AssertUnwindSafe(|| {
        let Some(host) = host() else {
            return Vec::new();
        };
        let default_name = default_device(&host, dir).map(|d| device_name(&d));
        devices(&host, dir)
            .iter()
            .map(|d| {
                let name = device_name(d);
                let r = ranges(d, dir);
                DeviceInfo {
                    is_default: default_name.as_deref() == Some(name.as_str()),
                    sample_rates: COMMON_RATES
                        .into_iter()
                        .filter(|rate| r.iter().any(|(lo, hi)| (lo..=hi).contains(&rate)))
                        .collect(),
                    channels: default_config(d, dir).map_or(0, |c| c.channels()),
                    name,
                }
            })
            .collect()
    }))
    .unwrap_or_default()
}

/// Output devices on the system. Empty (never a panic) when there is no host or device.
pub fn list_outputs() -> Vec<DeviceInfo> {
    list(Direction::Output)
}

/// Input devices on the system. Empty (never a panic) when there is no host or device.
pub fn list_inputs() -> Vec<DeviceInfo> {
    list(Direction::Input)
}

/// Pick the stream rate: `prefer` if any supported range contains it, otherwise `default`.
pub fn negotiate_rate(ranges: &[(u32, u32)], prefer: u32, default: u32) -> u32 {
    if ranges.iter().any(|&(lo, hi)| (lo..=hi).contains(&prefer)) {
        prefer
    } else {
        default
    }
}

/// A resolved device and the configuration to open it with.
pub(crate) struct Chosen {
    pub device: cpal::Device,
    pub name: String,
    pub config: cpal::StreamConfig,
    pub format: cpal::SampleFormat,
}

/// Find `name` (or the default device) and settle on a configuration, preferring `prefer_rate`.
pub(crate) fn choose(
    name: Option<&str>,
    dir: Direction,
    prefer_rate: Option<u32>,
) -> Result<Chosen, DeviceError> {
    catch_unwind(AssertUnwindSafe(|| choose_inner(name, dir, prefer_rate))).unwrap_or_else(|_| {
        Err(match name {
            Some(n) => DeviceError::NotFound(n.to_string()),
            None => DeviceError::NoDevice,
        })
    })
}

fn choose_inner(
    name: Option<&str>,
    dir: Direction,
    prefer_rate: Option<u32>,
) -> Result<Chosen, DeviceError> {
    let missing = || match name {
        Some(n) => DeviceError::NotFound(n.to_string()),
        None => DeviceError::NoDevice,
    };
    let host = host().ok_or_else(missing)?;
    let device = match name {
        Some(n) => devices(&host, dir)
            .into_iter()
            .find(|d| device_name(d) == n || d.id().is_ok_and(|id| id.to_string() == n)),
        None => default_device(&host, dir),
    }
    .ok_or_else(missing)?;
    let default = default_config(&device, dir)?;
    let rate = match prefer_rate {
        Some(p) => negotiate_rate(&ranges(&device, dir), p, default.sample_rate()),
        None => default.sample_rate(),
    };
    Ok(Chosen {
        name: device_name(&device),
        config: cpal::StreamConfig {
            channels: default.channels(),
            sample_rate: rate,
            buffer_size: cpal::BufferSize::Default,
        },
        format: default.sample_format(),
        device,
    })
}
