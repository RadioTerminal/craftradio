//! The output meter strips: what each output says about itself, and whether its level is real.

use radio_playout::{LocalOutput, OutputStatus};
use radio_stream::{StreamStatus, WebRtcStatus};
use radio_ui_egui::view::{OutputKind, OutputMeterView, Status};

/// What the station says when the Program device it wants did not open.
pub(crate) const CARD_NOT_OPEN: &str = "The sound card did not open: nothing plays the sound here";

/// True when the station wants a Program device and runs without one. No device by choice
/// is not a fault.
pub(crate) fn card_not_open(local: &LocalOutput, card_wanted: bool) -> bool {
    card_wanted && matches!(local, LocalOutput::Null)
}

/// The state of the local output for the status line.
pub(crate) fn local_status(local: &LocalOutput, card_wanted: bool) -> Status {
    match local {
        LocalOutput::Device { .. } => Status::Connected,
        LocalOutput::Lost { name } => Status::Failed(format!("Output device lost: {name}")),
        LocalOutput::Null if card_not_open(local, card_wanted) => {
            Status::Failed(CARD_NOT_OPEN.to_owned())
        }
        LocalOutput::Null => Status::NotConnected,
        LocalOutput::Offline => Status::Preview,
    }
}

/// The output strips in mixer order: the Program device, Icecast, WebRTC. An output is live
/// only while it really sends: a sound card is open, Icecast is connected, or somebody
/// listens over WebRTC.
pub(crate) fn output_meter_views(out: &OutputStatus, card_wanted: bool) -> Vec<OutputMeterView> {
    let device = match &out.local {
        LocalOutput::Device { name } => (true, name.clone()),
        LocalOutput::Lost { name } => (false, format!("Lost: {name}")),
        // A card that failed to open leaves the null output too: it is not the same as none.
        LocalOutput::Null | LocalOutput::Offline if card_wanted => (false, "Not open".to_owned()),
        LocalOutput::Null | LocalOutput::Offline => (false, "No device".to_owned()),
    };
    let icecast = match &out.icecast {
        None => (false, "Not set up"),
        Some(StreamStatus::Connected { .. }) => (true, "Connected"),
        Some(StreamStatus::Connecting) => (false, "Connecting"),
        Some(StreamStatus::Retrying { .. }) => (false, "Not connected"),
        Some(StreamStatus::Stopped) => (false, "Off"),
        Some(StreamStatus::OffAir) => (false, "Off air"),
    };
    let webrtc = match &out.webrtc {
        None | Some(WebRtcStatus::Stopped) => (false, "Off".to_owned()),
        Some(WebRtcStatus::Starting) => (false, "Starting".to_owned()),
        Some(WebRtcStatus::Failed(e)) => (false, format!("Failed: {e}")),
        Some(WebRtcStatus::Listening { listeners: 0, .. }) => {
            (false, "On — 0 listeners".to_owned())
        }
        // Listeners that pages report stay apart: the station cannot check them.
        Some(WebRtcStatus::Listening {
            listeners,
            reported,
            ..
        }) => {
            let direct = if *listeners == 1 {
                "1 listener".to_owned()
            } else {
                format!("{listeners} listeners")
            };
            match reported {
                0 => (true, direct),
                n => (true, format!("{direct}, {n} more reported by listeners")),
            }
        }
    };
    [
        (OutputKind::Device, "Sound card", device),
        (
            OutputKind::Icecast,
            "Icecast",
            (icecast.0, icecast.1.to_owned()),
        ),
        (OutputKind::WebRtc, "WebRTC", webrtc),
    ]
    .into_iter()
    .map(|(kind, label, (live, note))| OutputMeterView {
        kind,
        label: label.to_owned(),
        live,
        note,
        level_db: [radio_stream::LEVEL_FLOOR_DB; 2],
        peak_db: [radio_stream::LEVEL_FLOOR_DB; 2],
    })
    .collect()
}

/// The levels to show for `views`: a level only under a live output, silence otherwise.
pub(crate) fn gated_levels(views: &[OutputMeterView], levels: &[[f32; 2]]) -> Vec<[f32; 2]> {
    views
        .iter()
        .zip(levels)
        .map(|(v, l)| {
            if v.live {
                *l
            } else {
                [radio_stream::LEVEL_FLOOR_DB; 2]
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const FLOOR: f32 = radio_stream::LEVEL_FLOOR_DB;

    fn status(local: LocalOutput) -> OutputStatus {
        OutputStatus {
            local,
            icecast: None,
            webrtc: None,
            webrtc_available: true,
        }
    }

    fn view(out: &OutputStatus, kind: OutputKind) -> OutputMeterView {
        output_meter_views(out, false)
            .into_iter()
            .find(|v| v.kind == kind)
            .unwrap()
    }

    #[test]
    fn program_device_without_a_card_is_not_live() {
        for (local, note) in [
            (LocalOutput::Null, "No device"),
            (LocalOutput::Offline, "No device"),
            (
                LocalOutput::Lost {
                    name: "Studio out".into(),
                },
                "Lost: Studio out",
            ),
        ] {
            let v = view(&status(local), OutputKind::Device);
            assert!(!v.live);
            assert_eq!(v.note, note);
        }
        let v = view(
            &status(LocalOutput::Device {
                name: "Studio out".into(),
            }),
            OutputKind::Device,
        );
        assert!(v.live);
        assert_eq!(v.note, "Studio out");
    }

    #[test]
    fn a_wanted_card_that_is_not_open_does_not_read_as_no_device() {
        let out = status(LocalOutput::Null);
        let v = output_meter_views(&out, true).remove(0);
        assert!(!v.live);
        assert_eq!(v.note, "Not open");
        assert_eq!(output_meter_views(&out, false).remove(0).note, "No device");
    }

    #[test]
    fn no_device_by_choice_is_not_a_failure_but_a_card_that_did_not_open_is() {
        let chosen = local_status(&LocalOutput::Null, false);
        assert!(!matches!(chosen, Status::Failed(_) | Status::Connected));
        assert!(!card_not_open(&LocalOutput::Null, false));
        assert!(matches!(
            local_status(&LocalOutput::Null, true),
            Status::Failed(_)
        ));
        assert!(card_not_open(&LocalOutput::Null, true));
    }

    #[test]
    fn icecast_not_connected_is_not_live_and_says_so() {
        let mut out = status(LocalOutput::Null);
        out.icecast = Some(StreamStatus::Retrying {
            in_secs: 4,
            last_error: "refused".into(),
        });
        let v = view(&out, OutputKind::Icecast);
        assert!(!v.live);
        assert_eq!(v.note, "Not connected");
        out.icecast = None;
        assert_eq!(view(&out, OutputKind::Icecast).note, "Not set up");
    }

    #[test]
    fn webrtc_with_no_listeners_is_not_live() {
        let mut out = status(LocalOutput::Null);
        let listening = |listeners| WebRtcStatus::Listening {
            url: "http://127.0.0.1/".into(),
            public: None,
            listeners,
            since: jiff::Timestamp::UNIX_EPOCH,
            link: None,
            relays: 0,
            mapped: None,
            reported: 0,
        };
        out.webrtc = Some(listening(0));
        let v = view(&out, OutputKind::WebRtc);
        assert!(!v.live);
        assert_eq!(v.note, "On — 0 listeners");
        out.webrtc = Some(listening(2));
        let v = view(&out, OutputKind::WebRtc);
        assert!(v.live);
        assert_eq!(v.note, "2 listeners");
        out.webrtc = None;
        assert_eq!(view(&out, OutputKind::WebRtc).note, "Off");
    }

    #[test]
    fn webrtc_listeners_that_pages_report_never_add_to_the_direct_count() {
        let mut out = status(LocalOutput::Null);
        out.webrtc = Some(WebRtcStatus::Listening {
            url: "http://127.0.0.1/".into(),
            public: None,
            listeners: 3,
            since: jiff::Timestamp::UNIX_EPOCH,
            link: None,
            relays: 0,
            mapped: None,
            reported: 5,
        });
        let note = view(&out, OutputKind::WebRtc).note;
        assert!(note.starts_with("3 listeners"), "{note}");
        assert!(note.contains("5 more reported"), "{note}");
        assert!(!note.contains('8'), "never one sum: {note}");
    }

    #[test]
    fn a_live_output_passes_its_level_through_and_a_down_one_shows_silence() {
        let mut out = status(LocalOutput::Device {
            name: "Studio out".into(),
        });
        out.icecast = Some(StreamStatus::Connecting);
        let views = output_meter_views(&out, false);
        let levels = gated_levels(&views, &[[-6.0, -6.0], [-6.0, -6.0], [-6.0, -6.0]]);
        assert_eq!(levels, vec![[-6.0, -6.0], [FLOOR, FLOOR], [FLOOR, FLOOR]]);
    }
}
