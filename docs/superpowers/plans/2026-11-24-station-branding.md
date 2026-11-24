# Station Branding and Item Pictures Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A station sets its name and logo in Preferences, the listener page shows them when it has no song to show (app icon when no logo), and any item can carry its own picture.

**Architecture:** Branding is two settings in a `branding` part, set with `settings.set`. Playout watches them and hands name and logo path to the WebRTC server, which scales the logo on a thread and sends a `brand` message (plus `logo` parts) to each listener once, and serves the same at `GET /brand`. The page decides what to show. An item picture is a path in `ItemMeta`; the cover reader takes an image file as well as an audio file, so the existing cover and thumbnail path carries it.

**Tech Stack:** Rust (`image`, `lofty`, `serde_json`, `base64` already in `radio-stream`), egui, the one-file listener page.

**Spec:** `docs/superpowers/specs/2026-11-24-station-branding-design.md`

## Global Constraints

- No new dependencies. No `allow`. Non-test code does not panic.
- Engine state changes only through commands (`settings.set`, `item.set_meta`).
- UI sizes, gaps and colours come from `crates/ui-egui/src/theme.rs` tokens; each control's name starts with its visible label; the first field takes focus.
- Every setting lives in Preferences.
- A failed logo or picture never looks set: an alarm says why, and the Branding page and meta editor show the reason.
- `site/index.html` stays one file with no build step.
- Fixture names and titles are invented.
- Logo and picture files: PNG, JPEG or WebP, at most 4 MB, scaled to at most 512 px on the long edge.
- Empty station name means `"Radio"`.
- Integration tests go in each crate's `tests/it/`. Assert relations, not chosen values, unless the spec pins the value.
- One commit per task, plain English message, ending with `Co-Authored-By: Claude Fable 5.2 <noreply@anthropic.com>`.
- Checks per task for the crates it touches: `mise exec -- cargo fmt --all`, `mise exec -- cargo test -p <crates>`, `mise exec -- cargo clippy -p <crates> --all-targets -- -D warnings`.

## Review Focus

1. A station that has played nothing since start: a listener who connects still gets the brand (Task 2 test).
2. The branding changes while listeners are connected: they get the new brand, once (Task 2 test).
3. The logo path names a file that is not an image, or is missing: no logo is sent, the name still is, and an alarm carries the reason (Tasks 2 and 3 tests).
4. A newer page with an older station (no `brand`, no `/brand`), and an older page with a newer station: both look as today (Task 4 manual check).
5. A listener who plays through another listener and joins after the brand was sent: the relaying page passes the brand on (Task 4 manual check).

---

### Task 1: The station name is a setting

**Files:**
- Modify: `crates/model/src/lib.rs` (constants), `crates/engine/src/lib.rs` (`State::station_name`), `crates/playout/src/streams.rs`, `crates/ui-engine/src/lib.rs`, `apps/radio/src/main.rs`
- Test: `crates/playout/tests/it/streams.rs`, `crates/ui-engine/tests/it/snapshot.rs`

**Interfaces:**
- Produces:
  - `radio_model::BRANDING_PART: &str = "branding"`, `BRANDING_NAME_KEY = "name"`, `BRANDING_LOGO_KEY = "logo"`, `DEFAULT_STATION_NAME = "Radio"`.
  - `radio_engine::State::station_name(&self) -> Option<String>`: the trimmed `branding.name`, `None` when unset or blank.
  - `radio_engine::State::station_logo(&self) -> Option<PathBuf>`: `branding.logo`, `None` when unset or blank.

- [ ] **Step 1: Failing tests.**
  - `streams.rs`: `a_hidden_title_shows_the_station_name_from_the_settings`: dispatch `settings.set {part: "branding", key: "name", value: "Radio Lipa"}`, start an item with `PublishMode::Hide`, assert the title sent to the stream is `"Radio Lipa"`; then set the name to `"  "` and assert the next hidden item shows `cfg.station_name`.
  - `snapshot.rs`: the same through the view: a row that hides its title shows the branding name.
- [ ] **Step 2: Run them, expect failure** (the title is the configured name).
- [ ] **Step 3: Implement.** In `streams.rs` every `&self.cfg.station_name` becomes `self.engine.state().station_name().unwrap_or_else(|| self.cfg.station_name.clone())`. In `ui-engine` the uses of `self.station` (lines ~537, 541, 1141, 1257) do the same against the state. In `main.rs` drop `STATION_NAME`; pass `radio_model::DEFAULT_STATION_NAME` where the constant was, except `icecast_config`, which gets `engine.state().station_name()` or the default.
- [ ] **Step 4: Run the tests and the checks for `radio-model radio-engine radio-playout radio-ui-engine radio`.**
- [ ] **Step 5: Commit:** `The station name is a setting`.

### Task 2: The station sends its brand to listeners

**Files:**
- Create: `crates/stream/src/brand.rs`
- Modify: `crates/stream/src/lib.rs`, `crates/stream/src/now.rs` (`read_cover`), `crates/stream/src/webrtc.rs`
- Test: unit tests in `brand.rs`; `crates/stream/tests/it/webrtc.rs`

**Interfaces:**
- Produces:
  - `pub struct Picture { pub mime: String, pub data: Vec<u8> }`
  - `pub fn load_picture(path: &Path) -> Result<Picture, String>`: reads the file (error over 4 MB), decodes PNG/JPEG/WebP with `image`, scales to at most 512 px on the long edge, encodes PNG when it has alpha, else JPEG quality 85. Errors are plain sentences: `"the file does not exist"`, `"the file is larger than 4 MB"`, `"the file is not a PNG, JPEG or WebP picture"`.
  - `pub fn brand_messages(name: &str, id: u64, logo: Option<&Picture>) -> Vec<String>`: first `{"type":"brand","name":…,"logo":{"id","mime","parts"}|null}`, then one `{"type":"logo","id","part","data"}` per 48 KiB (the `PART` of `now.rs`, made `pub(crate)`).
  - `WebRtcHandle::set_brand(&self, name: String, logo: Option<PathBuf>)`: stores the name at once; a thread named `webrtc-brand` loads the logo, then stores the messages with a new brand revision. A load error goes to the error list and the brand is sent without a logo.
  - `WebRtcHandle::take_picture_errors(&self) -> Vec<PictureError>` with `pub struct PictureError { pub path: PathBuf, pub reason: String }`: drains errors from the logo and from item pictures.
  - `GET /brand` → `application/json`: `{"name":…,"logo":"data:<mime>;base64,…"|null}`; `404` before any `set_brand`. Add `/brand` to the 405 arm.
  - `now::read_cover(path)`: when the extension is `png`, `jpg`, `jpeg` or `webp`, returns `load_picture(path)` (an error goes to the error list); otherwise the tag cover as today.
- The brand lives beside `Now` in its own shared state (`rev`, `messages`). `Listener` gets `sent_brand: u64`, reset to 0 where `sent_rev` is reset; `send_now` queues the brand messages ahead of the item messages when `sent_brand` differs from the brand revision. The brand does not depend on `Now.messages`.

- [ ] **Step 1: Failing unit tests in `brand.rs`** (pictures made in the test with `image`):
  - `a_large_picture_is_scaled_down`: 2000×1000 PNG in → decoded output's long edge ≤ 512 and aspect kept within 1 px.
  - `a_picture_with_alpha_stays_png_and_one_without_becomes_jpeg`.
  - `a_text_file_is_not_a_picture` and `a_missing_file_says_so`: `Err`, non-empty reason.
  - `the_logo_parts_join_back_to_the_picture`: decode the `data` of the `logo` messages in `part` order → equals `Picture.data`; `brand.logo.parts` equals their count.
  - `a_brand_without_a_logo_is_one_message_with_a_null_logo`.
- [ ] **Step 2: Failing integration tests in `tests/it/webrtc.rs`** (follow the existing `/now` and data-channel tests there):
  - `brand_is_served_before_anything_plays`: start, `set_brand("Radio Lipa", None)`, `GET /brand` → 200, JSON name `"Radio Lipa"`, logo null; no `set_now` was called.
  - `a_listener_of_an_idle_station_gets_the_brand`: connect a listener with no `set_now`; it receives a `brand` message.
  - `a_changed_brand_reaches_a_connected_listener_once`: after a second `set_brand`, the listener gets exactly one more `brand` message.
  - `a_bad_logo_sends_the_name_and_reports_why`: `set_brand` with a text file → listener's `brand` has null logo; `take_picture_errors()` has one entry for that path, and a second call is empty.
- [ ] **Step 3: Run, expect failure. Step 4: Implement. Step 5: Run tests and checks for `radio-stream`.**
- [ ] **Step 6: Commit:** `The station sends its name and logo to listeners`.

### Task 3: Playout pushes the brand and raises picture alarms

**Files:**
- Modify: `crates/playout/src/streams.rs`, `crates/playout/src/core.rs` (tick), `crates/playout/src/alarm.rs`, plus wherever `Alarm` variants are named for the UI and CLI (`rg "Alarm::Unsupported"` lists them)
- Test: `crates/playout/tests/it/webrtc.rs`

**Interfaces:**
- Consumes: Task 1 `State::station_name`, `State::station_logo`; Task 2 `set_brand`, `take_picture_errors`.
- Produces: `Alarm::PictureFailed { path: String, reason: String }` (shown as "The picture {path} does not load: {reason}").
- `Core` keeps `brand_sent: Option<(String, Option<PathBuf>)>`. `sync_brand()`, called each tick after `sync_webrtc()`: when the server runs and (name, logo) differs from `brand_sent`, call `set_brand` and store it; when the server stops, clear `brand_sent`. Then drain `take_picture_errors()` and `raise` one `PictureFailed` each.

- [ ] **Step 1: Failing tests:**
  - `the_brand_follows_the_settings`: WebRTC on, set `branding.name`; within the test's tick budget `GET /brand` gives the new name; set it again → the new one.
  - `a_logo_that_does_not_load_raises_an_alarm`: set `branding.logo` to a text file → a `PictureFailed` alarm with that path appears in `alarms()`.
- [ ] **Step 2: Run, expect failure. Step 3: Implement. Step 4: Tests and checks for `radio-playout` and the crates that match on `Alarm`.**
- [ ] **Step 5: Commit:** `Playout sends the branding and says when a picture fails`.

### Task 4: The listener page shows the brand

**Files:**
- Modify: `site/index.html`
- Test: `crates/stream/tests/it/webrtc.rs` (the served page), manual check in the browser

**Interfaces:**
- Consumes: `brand` / `logo` messages and `GET /brand` from Task 2.

- [ ] **Step 1:** Replace the placeholder glyph in `.cover .placeholder` with the paths of `assets/app-icon/craftradio.svg`, inline; keep it `aria-hidden`.
- [ ] **Step 2:** Add brand state: `let brand = { name: "", logo: null }` (`logo` is a URL) and `setBrand(name, logoUrl)`. Split what is shown from what plays:
  - `setNow` keeps the item title in a variable; the shown title is item title, else `brand.name`, else `"Radio"`; `document.title` is the item title with artist, else `brand.name`, else `"Tune in"`.
  - `setCover` keeps the item cover; the shown picture is item cover, else `brand.logo`, else the placeholder. The backdrop blur uses the item cover only.
  - `updateSession`: `album` is `brand.name` or `"Live radio"`; `artwork` is the shown picture when there is one.
- [ ] **Step 3:** `onMessage`: `brand` sets the name and starts collecting parts (null logo clears it); `logo` parts join like `cover` parts. Neither sets `gotMeta`.
- [ ] **Step 4:** `fromParent`: keep `metaBrand = [text…]` (reset on each `brand`, `logo` parts appended) apart from `metaNow`; send it to a new child before `metaHistory`; clear it in `teardown`.
- [ ] **Step 5:** Before Listen: on load and in `tuneTo`, read `localStorage["brand:" + station.label]` (JSON `{name, logo}` with `logo` a data URL) and apply it; when `station.kind === "url"`, `fetch(station.url + "/brand")` and apply a 200 answer. On each complete brand, save it, skipping the logo when the JSON is over 300 000 characters. All storage calls are in `try`.
- [ ] **Step 6:** After Stop (`stop()`), clear the item title and cover so the brand shows again; `tuneTo` resets the brand before reading the new station's.
- [ ] **Step 7: Test** `the_served_page_knows_the_brand`: `GET /` contains `"/brand"` and `type === "brand"` (guards the embedded page against a stale build). Run `mise exec -- cargo test -p radio-stream`.
- [ ] **Step 8: Manual check** with `mise exec -- cargo run -p radio -- --webrtc 127.0.0.1:8000` and the page in a browser: brand before Listen; brand after Stop; logo as cover for an item without one; item cover wins; no logo → app icon; a station without `/brand` (stop the app, open the page from `site/` against an Icecast address or none) looks as today with the app icon.
- [ ] **Step 9: Commit:** `The listener page shows the station's name and logo`.

### Task 5: Preferences gets a Branding page

**Files:**
- Modify: `crates/ui-egui/src/preferences.rs`, `crates/ui-egui/src/parts.rs`, `crates/ui-egui/src/view.rs`, `crates/ui-egui/src/mock/` (the preview source), `crates/ui-engine/src/lib.rs`
- Test: unit tests in `preferences.rs` beside the existing ones; `crates/ui-engine/tests/it/snapshot.rs`

**Interfaces:**
- Consumes: Task 1 constants; `radio_stream::load_picture`.
- Produces:
  - `parts::BRANDING`, `parts::text(view, part, key) -> String`.
  - `view::check_picture(path: &str) -> Result<(), &'static str>`: Ok for an empty path; else the extension must be `png`, `jpg`, `jpeg` or `webp` (`"Pick a PNG, JPEG or WebP picture."`).
  - `StationView.branding_logo: Option<Result<Cover, String>>`: `None` with no logo set; the engine source fills it from `load_picture`, cached by path and modified time; the mock leaves `None`.
  - `Page::Branding`: label `"Branding"`, icon `Icon::Image`, group `"Streaming"`, description `"The name and logo that listeners see when no song shows."`, keywords `"station name logo picture brand listeners player"`, `restore` Ok (sets both keys to `""`).
- Form: section "Station": row "Station name" (text field, hint "Radio", takes focus when the page opens); row "Logo" (path field, "Browse…" with an image filter, "Clear"); under it the preview (a `size` token square, as the deck cover does) or the failure text in `t.danger` with a `TriangleAlert` icon, or the caption "No logo: listeners see the app icon." Edits go out as `parts::set(parts::BRANDING, key, value)` when the value changed.

- [ ] **Step 1: Failing tests:**
  - `preferences.rs`: `a_search_for_logo_finds_branding`; `restoring_branding_clears_the_name_and_the_logo` (the actions set both keys to empty).
  - `view.rs`: `a_picture_path_needs_a_picture_extension` (`"a.png"` and `""` Ok, `"a.mp3"` Err).
  - `snapshot.rs`: `a_logo_that_does_not_load_shows_why`: `branding.logo` set to a text file → `branding_logo` is `Some(Err(_))`; a real PNG → `Some(Ok(_))`; unset → `None`.
- [ ] **Step 2: Run, expect failure. Step 3: Implement** (update `Page::ALL` to 13 and every match). **Step 4: Tests and checks for `radio-ui-egui radio-ui-engine`.**
- [ ] **Step 5: Look at it:** `mise exec -- cargo run -p radio`, ⌘, → Branding; keyboard through the fields; set, clear and break the logo.
- [ ] **Step 6: Commit:** `Preferences has a Branding page for the station name and logo`.

### Task 6: An item can carry its own picture

**Files:**
- Modify: `crates/model/src/item.rs`, `crates/playout/src/streams.rs`, `crates/ui-egui/src/meta_editor.rs`, `crates/ui-egui/src/one_off.rs` (and `OneOffView` / the `Edit` that schedules a one-off in `view.rs`, with its handler in `crates/ui-engine/src/edit.rs`)
- Test: `crates/model/tests/model.rs`, `crates/playout/tests/it/webrtc.rs`, unit tests in `meta_editor.rs` and `one_off.rs`

**Interfaces:**
- Consumes: Task 2 `read_cover` taking image files; Task 5 `view::check_picture`.
- Produces: `ItemMeta.picture: String` (`#[serde(default)]` through the struct's default; empty means none).
- Playout, in `send_title` and `seed_played`: the cover file of an item is `meta.picture` when it is not empty, in the modes `FromTags`, `Custom` and `KeepPrevious`-resolved items alike; else the audio file as today (only `FromTags`). `Hide` never shows a picture.
- Meta editor: a "Picture" row (path, "Browse…", "Clear") with a `picture_touched` flag like `custom_touched`, merged the same way; the `check_picture` failure shows under the row and blocks Save.
- One-off dialog: the same row; the picture goes into the one-off's `ItemMeta`; editing a one-off fills it back.

- [ ] **Step 1: Failing tests:**
  - `model.rs`: `an_item_saved_before_pictures_loads_with_none` (JSON without `picture` → empty).
  - `webrtc.rs` (playout): `an_item_picture_is_its_cover`: an item with a Custom title and `meta.picture` set to a PNG airs → the listener's `now` message has a non-null `cover`; the same item with `Hide` → null. `a_broken_item_picture_raises_an_alarm`.
  - `meta_editor.rs`: `an_untouched_picture_keeps_each_items_own` (multi-edit merge).
  - `one_off.rs`: extend `editing_a_one_off_fills_the_fields_back` with the picture.
- [ ] **Step 2: Run, expect failure. Step 3: Implement. Step 4: Tests and checks for `radio-model radio-playout radio-ui-egui radio-ui-engine`.**
- [ ] **Step 5: Commit:** `A show or one-off can have its own picture`.

### Task 7: Whole-branch check

- [ ] `mise exec -- cargo fmt --all`, `mise exec -- cargo test --workspace`, `mise exec -- cargo clippy --workspace --all-targets -- -D warnings`.
- [ ] Repeat the manual checks of Tasks 4 and 5 once with a one-off live input that has a picture.
