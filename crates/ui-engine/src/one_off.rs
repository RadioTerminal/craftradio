//! One-off items of Upcoming as engine commands, and the engine one-offs as view rows.

use std::time::Duration;

use radio_engine::{Command, DEFAULT_TOLERANCE, OneOff};
use radio_model::{FixMode, Item, ItemId, Repeat, Source};
use radio_ui_egui::view::{OneOffRepeat, OneOffSource, OneOffView};

use crate::EngineSource;
use crate::edit::ProbeAfter;

fn repeat_of(r: Repeat) -> OneOffRepeat {
    match r {
        Repeat::Once => OneOffRepeat::Once,
        Repeat::Daily => OneOffRepeat::Daily,
        Repeat::Weekdays => OneOffRepeat::Weekdays,
        Repeat::Weekly => OneOffRepeat::Weekly,
    }
}

fn model_repeat(r: OneOffRepeat) -> Repeat {
    match r {
        OneOffRepeat::Once => Repeat::Once,
        OneOffRepeat::Daily => Repeat::Daily,
        OneOffRepeat::Weekdays => Repeat::Weekdays,
        OneOffRepeat::Weekly => Repeat::Weekly,
    }
}

/// The view of an engine one-off. A library item shows as its file.
pub(crate) fn view_of(o: &OneOff) -> OneOffView {
    let len = o.item.planned_len.unwrap_or_default();
    let source = match &o.item.source {
        Source::File { path } => OneOffSource::File { path: path.clone() },
        Source::Stream { url, keep_alive } => OneOffSource::Url {
            url: url.clone(),
            title: o.item.title.clone(),
            keep_alive: *keep_alive,
            len,
        },
        Source::LiveInput { device, channels } => OneOffSource::Live {
            device: device.clone(),
            channels: (u32::from(channels[0]) + 1, u32::from(channels[1]) + 1),
            len,
        },
    };
    OneOffView {
        id: o.item.id.0,
        source,
        start: o.start,
        hard: o.fix == FixMode::Hard,
        tolerance_s: o.tolerance.as_secs_f32(),
        repeat: repeat_of(o.repeat),
        picture: o.item.meta.picture.clone(),
    }
}

/// The engine command that adds the one-off `v` with `item`, or changes it.
pub(crate) fn command(v: &OneOffView, item: Item) -> Command {
    let fix = if v.hard { FixMode::Hard } else { FixMode::Soft };
    let tolerance = Duration::try_from_secs_f32(v.tolerance_s).unwrap_or(DEFAULT_TOLERANCE);
    let repeat = model_repeat(v.repeat);
    let one_off = Box::new(item);
    if v.id == 0 {
        Command::ScheduleAdd {
            one_off,
            start: v.start,
            fix,
            tolerance,
            repeat,
        }
    } else {
        Command::ScheduleUpdate {
            id: ItemId(v.id),
            one_off,
            start: v.start,
            fix,
            tolerance,
            repeat,
        }
    }
}

impl EngineSource {
    /// Adds or changes a one-off. The background lane reads a file first.
    pub(crate) fn schedule_one_off(&mut self, v: OneOffView) -> Result<usize, String> {
        let channel = |c: u32| {
            c.checked_sub(1)
                .and_then(|c| u16::try_from(c).ok())
                .ok_or_else(|| "No such input channel".to_owned())
        };
        let item = match &v.source {
            OneOffSource::File { path } => {
                self.queue_probe(vec![path.clone()], false, ProbeAfter::OneOff(v))?;
                return Ok(1);
            }
            OneOffSource::Library { item, title } => self
                .library_item(*item)
                .ok_or_else(|| format!("The library item {title} is gone"))?,
            OneOffSource::Url {
                url,
                title,
                keep_alive,
                len,
            } => {
                let host = radio_ui_egui::view::check_stream_url(url)?;
                let title = if title.trim().is_empty() {
                    host
                } else {
                    title.trim().to_owned()
                };
                let mut item = crate::plain_item(&title);
                item.source = Source::Stream {
                    url: url.trim().to_owned(),
                    keep_alive: *keep_alive,
                };
                item.planned_len = Some(*len);
                item
            }
            OneOffSource::Live {
                device,
                channels,
                len,
            } => {
                let title = format!("Live input · {device} ({}/{})", channels.0, channels.1);
                let mut item = crate::plain_item(&title);
                item.source = Source::LiveInput {
                    device: device.clone(),
                    channels: [channel(channels.0)?, channel(channels.1)?],
                };
                item.planned_len = Some(*len);
                item
            }
        };
        self.send_one_off(&v, item)?;
        Ok(1)
    }

    /// Sends the one-off with its item. A new one-off gets a fresh item id.
    pub(crate) fn send_one_off(&mut self, v: &OneOffView, mut item: Item) -> Result<(), String> {
        // The picture of the one-off always wins, empty too: the dialog starts with the
        // picture of a library item, and a cleared one stays cleared.
        item.meta.picture = v.picture.trim().to_owned();
        item.id = ItemId(if v.id == 0 { self.fresh_ids(1) } else { v.id });
        let what = format!(
            "Saved the one-off {} at {}",
            item.title,
            v.start.strftime("%Y-%m-%d %H:%M:%S")
        );
        self.send(&what, command(v, item))
    }
}
