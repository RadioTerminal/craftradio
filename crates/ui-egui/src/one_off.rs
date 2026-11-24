//! The one-off dialog of Upcoming. It picks a source, a local start time, the timing and a
//! repeat. The view source schedules the one-off.

use crate::chrome::{Answer, Dialog};
use crate::icons::Icon;
use crate::numbers;
use crate::widgets;
use std::time::Duration;

use egui::{DragValue, RichText};
use jiff::civil;
use jiff::{RoundMode, ToSpan, Unit};

use crate::add_menu::{self, DEFAULT_PLANNED_MIN, KEEP_ALIVE_LABEL};
use crate::app::Action;
use crate::theme::{self, Tokens, size, space};
use crate::view::{
    Edit, OneOffRepeat, OneOffSource, OneOffView, StationView, check_picture, check_stream_url,
};

/// The tolerance of a soft one-off that nobody set, in seconds (spec 6.3).
pub(crate) const DEFAULT_TOLERANCE_S: f32 = 1.0;
/// The widest tolerance of a soft one-off, in seconds.
const MAX_TOLERANCE_S: f32 = 3600.0;
/// The most library matches that the dialog lists.
const LIBRARY_MATCHES: usize = 50;
/// The default start is at least this far ahead.
const MIN_LEAD_MINUTES: i64 = 1;
const MAX_CHANNEL: u32 = 16;
/// The memory key that says the dialog was open in the last frame.
const OPEN: &str = "one_off_dialog_open";

/// True when the dialog was open in the last frame. Then Escape closes the dialog and does
/// not stop the carts. The call clears the mark.
pub(crate) fn escape_claimed(ctx: &egui::Context) -> bool {
    ctx.data_mut(|d| d.remove_temp::<bool>(egui::Id::unique(OPEN)))
        .unwrap_or(false)
}

/// The kinds of source, in the order of the dialog.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Kind {
    File,
    Library,
    Url,
    Live,
}

impl Kind {
    const ALL: [Self; 4] = [Self::File, Self::Library, Self::Url, Self::Live];

    fn label(self) -> &'static str {
        match self {
            Self::File => "File",
            Self::Library => "Library",
            Self::Url => "URL",
            Self::Live => "Live input",
        }
    }
}

/// The open one-off dialog.
pub(crate) struct OneOffDialog {
    /// The one-off that the dialog changes, or 0 for a new one.
    pub(crate) id: u64,
    pub(crate) kind: Kind,
    pub(crate) path: String,
    pub(crate) library: Option<(u64, String)>,
    pub(crate) search: String,
    pub(crate) url: String,
    pub(crate) title: String,
    pub(crate) keep_alive: bool,
    pub(crate) device: String,
    pub(crate) left: u32,
    pub(crate) right: u32,
    pub(crate) minutes: u32,
    pub(crate) seconds: u32,
    pub(crate) date: String,
    pub(crate) time: String,
    pub(crate) hard: bool,
    pub(crate) tolerance_s: f32,
    pub(crate) repeat: OneOffRepeat,
    /// The one-off's own picture file, or empty.
    pub(crate) picture: String,
    /// The picture that the last library pick filled in.
    filled: String,
    /// True after the first press of Add. Errors show from then on.
    tried: bool,
    /// The first field takes the keyboard focus once.
    focus: bool,
}

/// The next full hour at least a minute after `now`, in the time zone of `now`.
pub(crate) fn next_full_hour(now: &jiff::Zoned) -> civil::DateTime {
    let soon = now
        .datetime()
        .checked_add(MIN_LEAD_MINUTES.minutes())
        .unwrap_or_else(|_| now.datetime());
    let round = civil::DateTimeRound::new()
        .smallest(Unit::Hour)
        .mode(RoundMode::Ceil);
    soon.round(round).unwrap_or(soon)
}

impl OneOffDialog {
    /// A dialog for a new one-off: a file, at the next full hour, hard, once.
    pub(crate) fn new(view: &StationView) -> Self {
        let start = next_full_hour(&view.now);
        Self {
            id: 0,
            kind: Kind::File,
            path: String::new(),
            library: None,
            search: String::new(),
            url: "https://".to_owned(),
            title: String::new(),
            keep_alive: true,
            device: view.device_roles.input.clone(),
            left: crate::mixer::input_channels(view).0,
            right: crate::mixer::input_channels(view).1,
            minutes: DEFAULT_PLANNED_MIN,
            seconds: 0,
            date: start.date().to_string(),
            time: start.time().strftime("%H:%M:%S").to_string(),
            hard: true,
            tolerance_s: DEFAULT_TOLERANCE_S,
            repeat: OneOffRepeat::Once,
            picture: String::new(),
            filled: String::new(),
            tried: false,
            focus: true,
        }
    }

    /// A dialog that changes the one-off `o`.
    pub(crate) fn edit(o: &OneOffView, view: &StationView) -> Self {
        let mut d = Self::new(view);
        d.id = o.id;
        d.date = o.start.date().to_string();
        d.time = o.start.time().strftime("%H:%M:%S").to_string();
        d.hard = o.hard;
        d.tolerance_s = o.tolerance_s;
        d.repeat = o.repeat;
        d.picture.clone_from(&o.picture);
        let split = |len: Duration| {
            let s = u32::try_from(len.as_secs()).unwrap_or(u32::MAX);
            (s / 60, s % 60)
        };
        match &o.source {
            OneOffSource::File { path } => {
                d.kind = Kind::File;
                d.path.clone_from(path);
            }
            OneOffSource::Library { item, title } => {
                d.kind = Kind::Library;
                d.library = Some((*item, title.clone()));
            }
            OneOffSource::Url {
                url,
                title,
                keep_alive,
                len,
            } => {
                d.kind = Kind::Url;
                d.url.clone_from(url);
                d.title.clone_from(title);
                d.keep_alive = *keep_alive;
                (d.minutes, d.seconds) = split(*len);
            }
            OneOffSource::Live {
                device,
                channels,
                len,
            } => {
                d.kind = Kind::Live;
                d.device.clone_from(device);
                (d.left, d.right) = *channels;
                (d.minutes, d.seconds) = split(*len);
            }
        }
        d
    }

    /// Picks a library item. The picture follows the pick while nobody set one by hand.
    fn pick_library(&mut self, id: u64, title: String, picture: &str) {
        if self.library.as_ref().is_some_and(|(now, _)| *now == id) {
            return;
        }
        self.library = Some((id, title));
        if self.picture.is_empty() || self.picture == self.filled {
            self.picture = picture.to_owned();
            self.filled = picture.to_owned();
        }
    }

    fn len(&self) -> Duration {
        Duration::from_secs(u64::from(self.minutes) * 60 + u64::from(self.seconds))
    }

    fn source(&self, view: &StationView) -> Result<OneOffSource, String> {
        let length = || {
            let len = self.len();
            if len.is_zero() {
                Err("The planned length must be more than 0.".to_owned())
            } else {
                Ok(len)
            }
        };
        match self.kind {
            Kind::File => {
                let path = self.path.trim();
                if path.is_empty() {
                    return Err("Choose an audio file.".to_owned());
                }
                Ok(OneOffSource::File {
                    path: path.to_owned(),
                })
            }
            Kind::Library => match &self.library {
                Some((item, title)) => Ok(OneOffSource::Library {
                    item: *item,
                    title: title.clone(),
                }),
                None => Err("Pick an item of the library.".to_owned()),
            },
            Kind::Url => {
                check_stream_url(&self.url)?;
                Ok(OneOffSource::Url {
                    url: self.url.trim().to_owned(),
                    title: self.title.trim().to_owned(),
                    keep_alive: self.keep_alive,
                    len: length()?,
                })
            }
            Kind::Live => {
                if self.device.is_empty() || !view.input_devices.contains(&self.device) {
                    return Err("There is no input device. Connect one, then try again.".into());
                }
                Ok(OneOffSource::Live {
                    device: self.device.clone(),
                    channels: (self.left, self.right),
                    len: length()?,
                })
            }
        }
    }

    /// The one-off that the fields describe, and a note for a time that happens twice.
    pub(crate) fn check(&self, view: &StationView) -> Result<(OneOffView, Option<String>), String> {
        let source = self.source(view)?;
        check_picture(&self.picture).map_err(str::to_owned)?;
        let date: civil::Date = self
            .date
            .trim()
            .parse()
            .map_err(|_| "Write the date as YYYY-MM-DD.".to_owned())?;
        let time: civil::Time = self
            .time
            .trim()
            .parse()
            .map_err(|_| "Write the time as HH:MM or HH:MM:SS.".to_owned())?;
        let start = date.to_datetime(time);
        let tz = view.now.time_zone();
        let when = radio_timing::local_start(tz, start).map_err(|e| e.to_string())?;
        if self.repeat == OneOffRepeat::Once && when.first() <= view.now.timestamp() {
            return Err("That time has passed. Pick a later time.".to_owned());
        }
        if !(0.0..=MAX_TOLERANCE_S).contains(&self.tolerance_s) {
            return Err("The tolerance runs from 0 to 3600 s.".to_owned());
        }
        let note = matches!(when, radio_timing::LocalStart::Twice { .. }).then(|| {
            "The clocks go back over this time, so it happens twice. The one-off airs the \
             first time."
                .to_owned()
        });
        Ok((
            OneOffView {
                id: self.id,
                source,
                start,
                hard: self.hard,
                tolerance_s: if self.hard {
                    DEFAULT_TOLERANCE_S
                } else {
                    self.tolerance_s
                },
                repeat: self.repeat,
                picture: self.picture.trim().to_owned(),
            },
            note,
        ))
    }
}

/// A muted label for the first column of the grid.
fn label(ui: &mut egui::Ui, text: &str, t: &Tokens) -> egui::Response {
    ui.label(RichText::new(text).color(t.text_muted))
}

fn field(ui: &mut egui::Ui, text: &mut String, hint: &str, label: egui::Id) -> egui::Response {
    let edit = egui::TextEdit::singleline(text).hint_text(hint);
    ui.add_sized([size::FIELD_W * 2.0, size::CONTROL_SM], edit)
        .labelled_by(label)
}

fn source_rows(ui: &mut egui::Ui, d: &mut OneOffDialog, view: &StationView, t: &Tokens) {
    match d.kind {
        Kind::File => {
            let l = label(ui, "File", t);
            ui.horizontal(|ui| {
                let edit =
                    egui::TextEdit::singleline(&mut d.path).hint_text("path to an audio file");
                let r = ui
                    .add_sized(
                        [size::FIELD_W * 2.0 - size::COMBO_W / 2.0, size::CONTROL_SM],
                        edit,
                    )
                    .labelled_by(l.id);
                if d.focus {
                    r.request_focus();
                    d.focus = false;
                }
                if ui.button("Browse…").clicked()
                    && let Some(f) = rfd::FileDialog::new()
                        .add_filter("Audio", &crate::view::AUDIO_EXTENSIONS)
                        .pick_file()
                {
                    d.path = f.display().to_string();
                }
            });
            ui.end_row();
        }
        Kind::Library => {
            let l = label(ui, "Search library", t);
            let r = field(ui, &mut d.search, "title or artist", l.id);
            if d.focus {
                r.request_focus();
                d.focus = false;
            }
            ui.end_row();
            ui.label("");
            let needle = d.search.trim().to_lowercase();
            let matches: Vec<_> = view
                .library
                .iter()
                .filter(|i| {
                    needle.is_empty()
                        || i.title.to_lowercase().contains(&needle)
                        || i.artist
                            .as_deref()
                            .is_some_and(|a| a.to_lowercase().contains(&needle))
                })
                .take(LIBRARY_MATCHES)
                .collect();
            egui::ScrollArea::vertical()
                .id_salt("one_off_library")
                .max_height(size::EDITOR_ITEMS_H)
                .min_scrolled_height(size::EDITOR_ITEMS_H)
                .max_width(size::FIELD_W * 2.0)
                .show(ui, |ui| {
                    ui.vertical(|ui| {
                        ui.set_width(size::FIELD_W * 2.0);
                        if matches.is_empty() {
                            ui.label(RichText::new("No library item matches.").color(t.text_muted));
                        }
                        for item in matches {
                            let on = d.library.as_ref().is_some_and(|(id, _)| *id == item.id);
                            let text = match &item.artist {
                                Some(a) => {
                                    format!(
                                        "{} – {} · {}",
                                        a,
                                        item.title,
                                        crate::app::mmss(item.len)
                                    )
                                }
                                None => format!("{} · {}", item.title, crate::app::mmss(item.len)),
                            };
                            let r = widgets::steady(ui, on, egui::Button::new(text).truncate());
                            if r.clicked() {
                                let title = match &item.artist {
                                    Some(a) => format!("{a} – {}", item.title),
                                    None => item.title.clone(),
                                };
                                d.pick_library(item.id, title, &item.meta.picture);
                            }
                        }
                    })
                });
            ui.end_row();
        }
        Kind::Url => {
            let l = label(ui, "URL", t);
            let r = field(ui, &mut d.url, "https://", l.id);
            if d.focus {
                r.request_focus();
                d.focus = false;
            }
            ui.end_row();
            let l = label(ui, "Title", t);
            field(ui, &mut d.title, "optional", l.id);
            ui.end_row();
            add_menu::planned(ui, &mut d.minutes, &mut d.seconds, t);
            ui.label("");
            ui.checkbox(&mut d.keep_alive, KEEP_ALIVE_LABEL);
            ui.end_row();
        }
        Kind::Live => {
            label(ui, "Input device", t);
            craft_ui::ComboBox::from_id_salt("one_off_live_device")
                .width(size::COMBO_W)
                .selected_text(d.device.as_str())
                .show_ui(ui, |ui| {
                    for name in &view.input_devices {
                        widgets::selectable_value(ui, &mut d.device, name.clone(), name);
                    }
                });
            ui.end_row();
            label(ui, "Channels", t);
            ui.horizontal(|ui| {
                ui.add(numbers::field(
                    ui,
                    DragValue::new(&mut d.left)
                        .range(1..=MAX_CHANNEL)
                        .prefix("L "),
                ));
                ui.add(numbers::field(
                    ui,
                    DragValue::new(&mut d.right)
                        .range(1..=MAX_CHANNEL)
                        .prefix("R "),
                ));
            });
            ui.end_row();
            add_menu::planned(ui, &mut d.minutes, &mut d.seconds, t);
        }
    }
}

/// Shows the open one-off dialog. It closes after Add, Save or Cancel, or on Escape.
pub(crate) fn show(
    ctx: &egui::Context,
    slot: &mut Option<OneOffDialog>,
    view: &StationView,
    actions: &mut Vec<Action>,
    t: &Tokens,
) {
    let Some(d) = slot.as_mut() else {
        return;
    };
    ctx.data_mut(|data| data.insert_temp(egui::Id::unique(OPEN), true));
    let mut close = ctx.input(|i| i.key_pressed(egui::Key::Escape));
    let new = d.id == 0;
    let title = if new { "Add one-off" } else { "Edit one-off" };
    let word = if new { "Add" } else { "Save" };
    let mut checked = None;
    let mut answer = Dialog::new("one_off", Icon::CalendarClock, title)
        .ok(word, true)
        .width(size::DIALOG_WIDE_W)
        .show(
            ctx,
            t,
            |ui| {
                checked = Some(
                    egui::Grid::new("one_off_grid")
                        .num_columns(2)
                        .spacing(egui::vec2(space::M, space::S))
                        .show(ui, |ui| {
                            label(ui, "Source", t);
                            ui.horizontal(|ui| {
                                for k in Kind::ALL {
                                    let live = k == Kind::Live;
                                    let enabled = !live || !view.input_devices.is_empty();
                                    let on = d.kind == k;
                                    let r = ui
                                        .add_enabled_ui(enabled, |ui| {
                                            widgets::selectable(ui, on, k.label())
                                        })
                                        .inner;
                                    let name = format!("{} source", k.label());
                                    r.widget_info(|| {
                                        egui::WidgetInfo::selected(
                                            egui::Role::Button,
                                            enabled,
                                            d.kind == k,
                                            &name,
                                        )
                                    });
                                    let r = if live {
                                        r.on_disabled_hover_text("No input device is available")
                                    } else {
                                        r
                                    };
                                    if r.clicked() && d.kind != k {
                                        d.kind = k;
                                        d.focus = true;
                                    }
                                }
                            });
                            ui.end_row();
                            source_rows(ui, d, view, t);
                            let l = label(ui, "Date", t);
                            ui.horizontal(|ui| {
                                let edit =
                                    egui::TextEdit::singleline(&mut d.date).hint_text("YYYY-MM-DD");
                                ui.add_sized([size::COMBO_W, size::CONTROL_SM], edit)
                                    .labelled_by(l.id);
                                let l = label(ui, "Time", t);
                                let edit =
                                    egui::TextEdit::singleline(&mut d.time).hint_text("HH:MM:SS");
                                ui.add_sized(
                                    [size::COMBO_W / 2.0 + space::L, size::CONTROL_SM],
                                    edit,
                                )
                                .labelled_by(l.id);
                            });
                            ui.end_row();
                            ui.label("");
                            let zone = view.now.time_zone().iana_name().unwrap_or("station time");
                            ui.label(
                                RichText::new(format!("Local time in {zone}"))
                                    .font(theme::text(theme::CAPTION))
                                    .color(t.text_muted),
                            );
                            ui.end_row();
                            label(ui, "Timing", t);
                            ui.horizontal(|ui| {
                                ui.radio_value(&mut d.hard, true, "Hard — cuts in at the time");
                                ui.radio_value(&mut d.hard, false, "Soft — fitted");
                            });
                            ui.end_row();
                            let l = label(ui, "Tolerance", t);
                            ui.add_enabled_ui(!d.hard, |ui| {
                                ui.add(numbers::field(
                                    ui,
                                    DragValue::new(&mut d.tolerance_s)
                                        .range(0.0..=MAX_TOLERANCE_S)
                                        .speed(0.1)
                                        .fixed_decimals(1)
                                        .prefix("± ")
                                        .suffix(" s"),
                                ))
                                .labelled_by(l.id);
                            })
                            .response
                            .on_disabled_hover_text("A hard one-off starts at its time sharp");
                            ui.end_row();
                            label(ui, "Repeat", t);
                            craft_ui::ComboBox::from_id_salt("one_off_repeat")
                                .width(size::COMBO_W)
                                .selected_text(d.repeat.label())
                                .show_ui(ui, |ui| {
                                    for r in OneOffRepeat::ALL {
                                        widgets::selectable_value(ui, &mut d.repeat, r, r.label());
                                    }
                                });
                            ui.end_row();
                            let l = label(ui, "Picture", t);
                            widgets::picture_field(
                                ui,
                                &mut d.picture,
                                "optional, shown to listeners",
                                l.id,
                                true,
                                t,
                            );
                            ui.end_row();
                            let checked = d.check(view);
                            let line = match &checked {
                                Err(e) if d.tried => Some((e.clone(), t.danger)),
                                Ok((_, Some(note))) => Some((note.clone(), t.warn)),
                                _ => None,
                            };
                            if let Some((text, colour)) = line {
                                ui.label("");
                                ui.label(
                                    RichText::new(text)
                                        .font(theme::text(theme::CAPTION))
                                        .color(colour),
                                );
                                ui.end_row();
                            }
                            checked
                        })
                        .inner,
                );
            },
            |_| {},
        );
    if ctx.input(|i| i.key_pressed(egui::Key::Enter) && i.modifiers.command) {
        answer = Some(Answer::Ok);
    }
    match answer {
        Some(Answer::Ok) => match &checked {
            Some(Ok((one_off, _))) => {
                actions.push(Action::Edit(Edit::ScheduleOneOff(one_off.clone())));
                close = true;
            }
            _ => d.tried = true,
        },
        Some(Answer::Cancel) => close = true,
        None => {}
    }
    if close {
        *slot = None;
        // The fields of the dialog are gone, so the lists get the keys again.
        ctx.memory_mut(|m| {
            if let Some(id) = m.focused() {
                m.surrender_focus(id);
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::view::ViewSource;

    fn view_at(now: &str, tz: &str) -> StationView {
        let tz = jiff::tz::TimeZone::get(tz).expect("tz");
        let mut v = crate::MockSource::new(tz).snapshot(now.parse().expect("ts"));
        v.input_devices.clear();
        v
    }

    fn url_dialog(view: &StationView) -> OneOffDialog {
        let mut d = OneOffDialog::new(view);
        d.kind = Kind::Url;
        d.url = "https://relay.example.org/live".to_owned();
        d
    }

    #[test]
    fn a_new_one_off_starts_at_the_next_full_hour_and_passes_the_check() {
        let view = view_at("2026-11-21T13:58:30Z", "Europe/Ljubljana");
        let d = url_dialog(&view);
        let (o, note) = d.check(&view).expect("the defaults are valid");
        assert!(note.is_none());
        assert_eq!(o.start.time().minute(), 0);
        assert_eq!(o.start.time().second(), 0);
        let start = o.start.to_zoned(view.now.time_zone().clone()).unwrap();
        assert!(start > view.now, "the default is in the future");
        assert!(
            start.timestamp().duration_since(view.now.timestamp())
                <= jiff::SignedDuration::from_hours(1),
            "and within the next hour"
        );
    }

    #[test]
    fn a_time_in_the_dst_gap_is_an_error_and_a_fold_gives_a_note() {
        let view = view_at("2026-10-24T12:00:00Z", "Europe/Ljubljana");
        let mut d = url_dialog(&view);
        d.date = "2027-03-28".to_owned();
        d.time = "02:30".to_owned();
        let err = d.check(&view).unwrap_err();
        assert!(err.contains("does not exist"), "{err}");
        d.date = "2026-10-25".to_owned();
        let (_, note) = d.check(&view).expect("a fold is a valid time");
        assert!(note.is_some_and(|n| n.contains("twice")));
    }

    #[test]
    fn a_past_time_and_a_missing_source_are_errors() {
        let view = view_at("2026-11-21T13:58:30Z", "Europe/Ljubljana");
        let mut d = url_dialog(&view);
        d.date = "2026-11-21".to_owned();
        d.time = "08:00".to_owned();
        assert!(d.check(&view).unwrap_err().contains("passed"));
        d.repeat = OneOffRepeat::Daily;
        assert!(d.check(&view).is_ok(), "a repeat may start in the past");
        d.kind = Kind::File;
        assert!(d.check(&view).is_err(), "a file one-off needs a file");
        d.kind = Kind::Live;
        assert!(d.check(&view).is_err(), "no input device, no live one-off");
    }

    #[test]
    fn editing_a_one_off_fills_the_fields_back() {
        let view = view_at("2026-11-21T13:58:30Z", "Europe/Ljubljana");
        let file =
            std::env::temp_dir().join(format!("craftradio-one-off-{}.png", std::process::id()));
        std::fs::write(&file, b"x").unwrap();
        let picture = file.to_string_lossy().into_owned();
        let (o, _) = {
            let mut d = url_dialog(&view);
            d.hard = false;
            d.tolerance_s = 2.5;
            d.repeat = OneOffRepeat::Weekly;
            d.picture.clone_from(&picture);
            d.check(&view).unwrap()
        };
        let mut o = o;
        o.id = 9;
        let back = OneOffDialog::edit(&o, &view).check(&view).unwrap().0;
        assert_eq!(back, o);
        assert_eq!(back.picture, picture, "the picture comes back");
        let _ = std::fs::remove_file(&file);
    }

    #[test]
    fn a_library_pick_fills_the_picture_until_it_is_set_by_hand() {
        let view = view_at("2026-11-21T13:58:30Z", "Europe/Ljubljana");
        let mut d = OneOffDialog::new(&view);
        d.pick_library(1, "One".into(), "/art/one.png");
        assert_eq!(d.picture, "/art/one.png");
        d.pick_library(2, "Two".into(), "");
        assert_eq!(d.picture, "", "it follows an item without a picture");
        d.pick_library(1, "One".into(), "/art/one.png");
        d.pick_library(1, "One".into(), "/art/other.png");
        assert_eq!(d.picture, "/art/one.png", "the same item changes nothing");
        d.picture = "/mine.webp".into();
        d.pick_library(3, "Three".into(), "/art/three.png");
        assert_eq!(d.picture, "/mine.webp", "a picture set by hand stays");
        assert_eq!(d.library.as_ref().map(|l| l.0), Some(3));
    }

    #[test]
    fn a_picture_that_is_not_one_stops_the_one_off() {
        let view = view_at("2026-11-21T13:58:30Z", "Europe/Ljubljana");
        let mut d = url_dialog(&view);
        d.picture = "/art/show.mp3".to_owned();
        assert_eq!(
            d.check(&view).unwrap_err(),
            check_picture("/art/show.mp3").unwrap_err()
        );
        d.picture.clear();
        assert!(d.check(&view).is_ok());
    }
}
