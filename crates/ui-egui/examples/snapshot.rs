//! Render the preview to PNG files without opening a window.
//!
//! Usage: `cargo run -p radio-ui-egui --example snapshot -- [out-dir] [RFC3339 instant]`
//! The defaults are `docs/screenshots` and `2026-11-21T13:58:00Z`.
//! Here egui draws tool windows as floating windows over the main window.
//!
//! `snapshot -- qa [out-dir]` renders the visual check set instead. It shows each window and
//! state at three widths, with the normal and the stress data, in `docs/screenshots/qa`.

use std::process::ExitCode;

use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use jiff::Timestamp;
use jiff::tz::TimeZone;
use radio_ui_egui::mock::StressSource;
use radio_ui_egui::theme::size;
use radio_ui_egui::view::{Edit, Row, StationView};
use radio_ui_egui::{MockSource, RadioApp, ToolWindow, ViewSource};

/// Render at twice the point size, as a Retina display does.
const PIXELS_PER_POINT: f32 = 2.0;
/// The check set renders at the point size, which keeps the files small.
const QA_PIXELS_PER_POINT: f32 = 1.0;
const FRAMES: usize = 6;
/// The widths of the main window in the check set.
const QA_WIDTHS: [f32; 3] = [1024.0, 1440.0, 1920.0];

/// What to open before a picture.
#[derive(Clone)]
enum Scene {
    Main,
    Window(ToolWindow),
    /// A tool window at the minimum size of its kind.
    WindowMin(ToolWindow),
    Click(Vec<String>),
    /// Open a window, then click labels in it.
    WindowClick(ToolWindow, Vec<String>),
    /// Right-click the lowest node with this label, then click the menu row, if any.
    Menu(String, String),
    /// Open a window, then right-click a node in it.
    WindowMenu(ToolWindow, String),
}

const ROW: &str = "Juniper Vale – Satellites of Ulvenik";

/// A source that adds settings for the check set: a tool window at its minimum size.
struct Qa {
    inner: Box<dyn ViewSource>,
    min: Option<ToolWindow>,
}

impl ViewSource for Qa {
    fn snapshot(&mut self, now: Timestamp) -> StationView {
        let mut v = self.inner.snapshot(now);
        if let Some(w) = self.min {
            let [x, y] = w.min_size();
            v.settings
                .entry(w.part().to_owned())
                .or_default()
                .insert("window_size".to_owned(), serde_json::json!([x, y]));
        }
        v
    }

    fn edit(&mut self, edit: Edit) -> Result<usize, String> {
        self.inner.edit(edit)
    }

    fn stream_title(&self, row: &Row) -> String {
        self.inner.stream_title(row)
    }
}

fn source(tz: &TimeZone, stress: bool) -> Box<dyn ViewSource> {
    if stress {
        Box::new(StressSource::new(tz.clone()))
    } else {
        Box::new(MockSource::new(tz.clone()))
    }
}

fn main() -> ExitCode {
    let mut args = std::env::args().skip(1).peekable();
    let qa = args.peek().is_some_and(|a| a == "qa");
    if qa {
        args.next();
    }
    let default_dir = if qa {
        "docs/screenshots/qa"
    } else {
        "docs/screenshots"
    };
    let dir = args.next().unwrap_or_else(|| default_dir.to_owned());
    let at = args
        .next()
        .unwrap_or_else(|| "2026-11-21T13:58:00Z".to_owned());
    let Ok(now) = at.parse::<Timestamp>() else {
        eprintln!("snapshot: {at:?} is not an RFC 3339 instant");
        return ExitCode::FAILURE;
    };
    let Ok(tz) = TimeZone::get("Europe/Ljubljana") else {
        eprintln!("snapshot: no time zone database");
        return ExitCode::FAILURE;
    };
    if qa {
        return qa_set(&dir, &tz, now);
    }
    let l = |s: &[&str]| s.iter().map(|x| (*x).to_owned()).collect::<Vec<_>>();
    let scenes = [
        ("ui-preview", Scene::Main),
        ("cartwall", Scene::Window(ToolWindow::Cartwall)),
        ("mixer", Scene::Window(ToolWindow::Mixer)),
        (
            "mixer-settings",
            Scene::Click(l(&["Mixer", "Toggle Mixer settings"])),
        ),
        ("clock", Scene::Window(ToolWindow::Clock)),
        ("history", Scene::Window(ToolWindow::History)),
        ("library", Scene::Window(ToolWindow::Library)),
        ("rules-editor", Scene::Click(l(&["Music rules"]))),
        ("rules-script", Scene::Click(l(&["Music rules", "Script"]))),
        ("context-menu", Scene::Menu(ROW.into(), String::new())),
        ("add-menu", Scene::Click(l(&["Add to Music"]))),
        ("add-url", Scene::Click(l(&["Add to Music", "Add URL…"]))),
        ("metadata", Scene::Menu(ROW.into(), "Edit metadata".into())),
    ];
    for (name, scene) in scenes {
        let src = source(&tz, false);
        let out = format!("{dir}/{name}.png");
        let size = egui::vec2(size::WINDOW_W, size::WINDOW_H);
        if let Err(e) = render(src, None, now, size, PIXELS_PER_POINT, &scene, &out) {
            eprintln!("snapshot: {e}");
            return ExitCode::FAILURE;
        }
    }
    ExitCode::SUCCESS
}

/// The check set: every window and state with both data sets.
fn qa_set(dir: &str, tz: &TimeZone, now: Timestamp) -> ExitCode {
    if let Err(e) = std::fs::create_dir_all(dir) {
        eprintln!("snapshot: cannot make {dir}: {e}");
        return ExitCode::FAILURE;
    }
    let only = std::env::var("QA_ONLY").ok();
    let mut failed = 0;
    for (data, stress) in [("normal", false), ("stress", true)] {
        let view = source(tz, stress).snapshot(now);
        let main = view.main_playlist.clone();
        let row = view
            .rows
            .iter()
            .find(|r| r.title.chars().count() > 20)
            .map_or_else(|| ROW.to_owned(), |r| r.title.clone());
        let deck: String = view.decks[0].title.chars().take(10).collect();
        let cart = view
            .carts
            .iter()
            .find(|c| !c.is_empty())
            .map(|c| c.chars().take(6).collect())
            .unwrap_or_default();
        let lib = view
            .library
            .first()
            .map(|l| l.title.clone())
            .unwrap_or_default();
        let mut scenes: Vec<(String, f32, Scene)> = Vec::new();
        for w in QA_WIDTHS {
            scenes.push(("main".into(), w, Scene::Main));
        }
        let wide = size::WINDOW_W;
        for tab in &view.tabs {
            let slug: String = tab
                .name
                .chars()
                .take_while(|c| c.is_alphanumeric())
                .collect::<String>()
                .to_lowercase();
            scenes.push((
                format!("tab-{slug}"),
                wide,
                Scene::Click(vec![tab.name.chars().take(12).collect()]),
            ));
        }
        for w in ToolWindow::ALL {
            let name = w.title().to_lowercase();
            scenes.push((name.clone(), wide, Scene::Window(w)));
            scenes.push((format!("{name}-min"), wide, Scene::WindowMin(w)));
        }
        let wc = |w: ToolWindow, label: &str| Scene::WindowClick(w, vec![label.to_owned()]);
        scenes.push((
            "mixer-settings".into(),
            wide,
            wc(ToolWindow::Mixer, "Toggle Mixer settings"),
        ));
        scenes.push((
            "cartwall-settings".into(),
            wide,
            wc(ToolWindow::Cartwall, "Toggle Cartwall settings"),
        ));
        scenes.push((
            "library-settings".into(),
            wide,
            wc(ToolWindow::Library, "Toggle Library settings"),
        ));
        let rules = format!("{main} rules");
        scenes.push((
            "rules-visual".into(),
            wide,
            Scene::Click(vec![rules.clone()]),
        ));
        scenes.push((
            "rules-script".into(),
            wide,
            Scene::Click(vec![rules, "Script".into()]),
        ));
        let add = format!("Add to {main}");
        scenes.push(("add-menu".into(), wide, Scene::Click(vec![add.clone()])));
        scenes.push((
            "add-url".into(),
            wide,
            Scene::Click(vec![add.clone(), "Add URL…".into()]),
        ));
        scenes.push((
            "add-live".into(),
            wide,
            Scene::Click(vec![add, "Add live input…".into()]),
        ));
        scenes.push((
            "row-menu".into(),
            wide,
            Scene::Menu(row.clone(), String::new()),
        ));
        scenes.push((
            "metadata".into(),
            wide,
            Scene::Menu(row, "Edit metadata".into()),
        ));
        scenes.push(("deck-menu".into(), wide, Scene::Menu(deck, String::new())));
        scenes.push((
            "cart-menu".into(),
            wide,
            Scene::WindowMenu(ToolWindow::Cartwall, cart),
        ));
        scenes.push((
            "library-menu".into(),
            wide,
            Scene::WindowMenu(ToolWindow::Library, lib),
        ));
        scenes.push((
            "deck-armed".into(),
            wide,
            Scene::Click(vec!["Play deck B".into()]),
        ));
        scenes.push((
            "toast".into(),
            QA_WIDTHS[0],
            Scene::Click(vec!["Stop after current".into()]),
        ));
        for (name, width, scene) in scenes {
            if only.as_ref().is_some_and(|o| !name.starts_with(o.as_str())) {
                continue;
            }
            let min = match &scene {
                Scene::WindowMin(w) => Some(*w),
                _ => None,
            };
            let out = format!("{dir}/{name}-{}-{data}.png", width as u32);
            let size = egui::vec2(width, size::WINDOW_H);
            let src = source(tz, stress);
            if let Err(e) = render(src, min, now, size, QA_PIXELS_PER_POINT, &scene, &out) {
                eprintln!("snapshot: {e}");
                failed += 1;
            }
        }
    }
    if failed == 0 {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

fn click(harness: &mut Harness<'_, RadioApp>, label: &str) -> Result<(), String> {
    let node = harness
        .query_all_by_label(label)
        .next()
        .or_else(|| harness.query_all_by_label_contains(label).next())
        .ok_or_else(|| format!("no control {label:?}"))?;
    node.click();
    harness.run_steps(FRAMES);
    Ok(())
}

fn right_click(harness: &mut Harness<'_, RadioApp>, label: &str) -> Result<(), String> {
    let node = harness
        .query_all_by_label_contains(label)
        .max_by(|a, b| a.rect().top().total_cmp(&b.rect().top()))
        .ok_or_else(|| format!("no node {label:?}"))?;
    node.click_secondary();
    harness.run_steps(FRAMES);
    Ok(())
}

fn render(
    src: Box<dyn ViewSource>,
    min: Option<ToolWindow>,
    now: Timestamp,
    size: egui::Vec2,
    ppp: f32,
    scene: &Scene,
    out: &str,
) -> Result<(), String> {
    let source = Qa { inner: src, min };
    let mut harness = Harness::builder()
        .with_size(size)
        .with_pixels_per_point(ppp)
        .wgpu()
        .build_eframe(move |_cc| RadioApp::new(Box::new(source), Box::new(move || now)));
    harness.run_steps(FRAMES);
    match scene {
        Scene::Main => {}
        Scene::Window(w) | Scene::WindowMin(w) => harness.state_mut().set_window(*w, true),
        Scene::WindowClick(w, labels) => {
            harness.state_mut().set_window(*w, true);
            harness.run_steps(FRAMES);
            for label in labels {
                click(&mut harness, label)?;
            }
        }
        Scene::WindowMenu(w, label) => {
            harness.state_mut().set_window(*w, true);
            harness.run_steps(FRAMES);
            right_click(&mut harness, label)?;
        }
        Scene::Menu(row, item) => {
            right_click(&mut harness, row)?;
            if !item.is_empty() {
                let node = harness
                    .query_all_by_label_contains(item)
                    .next()
                    .ok_or_else(|| format!("no menu row {item:?}"))?;
                node.click();
                harness.run_steps(FRAMES);
            }
        }
        Scene::Click(labels) => {
            for label in labels {
                click(&mut harness, label)?;
            }
        }
    }
    harness.run_steps(FRAMES);
    let image = harness
        .render()
        .map_err(|e| format!("render failed for {out}: {e}"))?;
    image
        .save(out)
        .map_err(|e| format!("could not write {out}: {e}"))?;
    println!("snapshot: wrote {out}");
    Ok(())
}
