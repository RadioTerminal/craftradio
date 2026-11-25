//! Updates, as in the Craft apps: a quiet check at start, Help ▸ Check for Updates, the
//! update prompt and the self-update. craft-update does the work and draws the dialogs.

use std::sync::Arc;

use craft_update::http::Fetched;
use craft_update::updater::{Action, CheckSource, Updater};
use craft_update::{App, CheckResultDialog, CheckResultResponse, Http, Release, version};

/// CraftRadio as its GitHub releases know it.
pub static APP: App = App {
    developer: "RadioTerminal",
    // The id in the app's Info.plist (packaging/macos/Info.plist.in). Without it the updater
    // looks for `ai.storyteller.craftradio`, does not know the installed app, and offers only
    // Download.
    bundle_id_override: Some("io.github.radioterminal.craftradio"),
    ..App::new(
        "craftradio",
        crate::menus::APP_NAME,
        "Radio station manager",
        "RadioTerminal/craftradio",
    )
};

/// The version of this build. The release workflow writes it into the workspace.
const VERSION: &str = env!("CARGO_PKG_VERSION");

/// True when the version `running` is itself a published prerelease (`0.2.0-rc.1`). A build
/// from source has a suffix too, and is not one.
fn suffixed_prerelease(running: &str) -> bool {
    version::parse(running).is_some_and(|v| !version::is_stable(&v) && !version::is_local_build(&v))
}

/// True when GitHub's release list `json` marks the release of the version `running` as a
/// prerelease. A release can be one without a suffix in its version (`v0.1.3`, published with
/// "Set as a pre-release"), and only the list tells.
fn published_as_prerelease(json: &str, running: &str) -> bool {
    let Some(running) = version::parse(running) else {
        return false;
    };
    let list: serde_json::Value = serde_json::from_str(json).unwrap_or_default();
    list.as_array().is_some_and(|list| {
        list.iter().any(|item| {
            let tag = item["tag_name"].as_str().unwrap_or_default();
            let same = version::parse(tag.trim_start_matches('v'))
                .is_some_and(|v| version::compare(&v, &running).is_eq());
            same && item["prerelease"].as_bool().unwrap_or(false)
        })
    })
}

/// The release in GitHub's release list `json` to offer a copy running `running`. A copy of a
/// prerelease is offered the newest published release of either kind, so it follows the next
/// prerelease and then the stable release. Any other copy is offered stable releases only.
fn offered(json: &str, running: &str) -> Result<Option<Release>, String> {
    let prereleases = suffixed_prerelease(running) || published_as_prerelease(json, running);
    let newest = craft_update::parse_github_release_list(json, APP.repo, prereleases)?;
    let running = version::parse(running);
    Ok(newest.filter(|release| {
        release
            .version
            .as_ref()
            .zip(running.as_ref())
            .is_some_and(|(newest, running)| version::offers_update(newest, running))
    }))
}

/// The update check. craft-update's own check offers stable releases only, so a copy of a
/// prerelease would never hear of the next one.
fn check(running: String) -> CheckSource {
    Arc::new(move || {
        let http = Http::new(&format!("{}/Update", APP.name))?;
        let url = format!(
            "https://api.github.com/repos/{}/releases?per_page=30",
            APP.repo
        );
        match http.get_api(&url, None)? {
            Fetched::Body { text, .. } => offered(&text, &running),
            Fetched::RateLimited { .. } => Err("GitHub's request limit was reached".into()),
            Fetched::NotModified | Fetched::NotFound => Ok(None),
        }
    })
}

/// The updater and the card that tells what a requested check found.
pub(crate) struct Updates {
    updater: Updater,
    result: CheckResultDialog,
}

impl Updates {
    pub(crate) fn new() -> Self {
        // The version this copy counts as, which a test run can set.
        let running = craft_update::self_update::current_version(&APP, VERSION)
            .map_or_else(|| VERSION.to_owned(), |v| v.to_string());
        Self {
            updater: Updater::new(&APP, VERSION).with_source(check(running)),
            result: CheckResultDialog::new(&APP, VERSION),
        }
    }

    /// Help ▸ Check for Updates.
    pub(crate) fn check_now(&mut self, ctx: &egui::Context) {
        self.updater.check_now(ctx);
    }

    /// True while the prompt or a modal result card is open. The menus run nothing then.
    /// The notice after an update leaves the app usable.
    pub(crate) fn modal(&self) -> bool {
        use craft_update::updater::Status;
        self.updater.prompt_open()
            || !matches!(self.updater.status(), Status::Idle | Status::Updated(_))
    }

    /// Runs the updater for one frame and draws its dialogs.
    pub(crate) fn show(&mut self, ctx: &egui::Context) {
        self.updater.check_at_start(ctx);
        self.updater.tick(ctx);
        match self.updater.show(ctx) {
            Action::None => {}
            Action::OpenUrl(url) => ctx.open_url(egui::OpenUrl::new_tab(url)),
            Action::Close => ctx.send_viewport_cmd(egui::ViewportCommand::Close),
        }
        // A release found goes to the prompt; the card shows the other outcomes.
        if self.updater.prompt_open() {
            return;
        }
        let status = self.updater.status().clone();
        match self.result.show(ctx, &status) {
            CheckResultResponse::None => {}
            CheckResultResponse::Close => self.updater.dismiss_status(),
            CheckResultResponse::CheckAgain => self.updater.check_now(ctx),
            CheckResultResponse::RestartToUpdate => self.updater.restart_to_update(),
        }
    }

    /// Starts the helper of a ready update. The app calls it when it exits.
    pub(crate) fn exit(&mut self) {
        self.updater.start_pending_update();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Releases as this repository publishes them: prereleases without a suffix.
    const MARKED: &str = r#"[
        {"tag_name":"v0.1.4","prerelease":true,"draft":false,"assets":[]},
        {"tag_name":"v0.1.3","prerelease":true,"draft":false,"assets":[]},
        {"tag_name":"v0.1.0","prerelease":false,"draft":false,"assets":[]}]"#;

    const SUFFIXED: &str = r#"[
        {"tag_name":"v0.3.0-rc.2","prerelease":true,"draft":false,"assets":[]},
        {"tag_name":"v0.3.0-rc.1","prerelease":true,"draft":false,"assets":[]},
        {"tag_name":"v0.2.0","prerelease":false,"draft":false,"assets":[]}]"#;

    fn tag(list: &str, running: &str) -> Option<String> {
        offered(list, running)
            .expect("the list reads")
            .map(|release| release.tag)
    }

    #[test]
    fn a_copy_of_a_marked_prerelease_is_offered_the_next_one() {
        assert_eq!(tag(MARKED, "0.1.3").as_deref(), Some("v0.1.4"));
        assert_eq!(tag(MARKED, "0.1.4"), None, "the newest has no update");
    }

    #[test]
    fn a_stable_copy_is_not_offered_a_prerelease() {
        assert_eq!(tag(MARKED, "0.1.0"), None);
        assert_eq!(tag(SUFFIXED, "0.2.0"), None);
    }

    #[test]
    fn a_suffixed_prerelease_copy_is_offered_the_next_one() {
        assert_eq!(tag(SUFFIXED, "0.3.0-rc.1").as_deref(), Some("v0.3.0-rc.2"));
    }

    #[test]
    fn a_prerelease_copy_is_offered_the_stable_release_after_it() {
        let list = r#"[
            {"tag_name":"v0.3.0","prerelease":false,"draft":false,"assets":[]},
            {"tag_name":"v0.3.0-rc.2","prerelease":true,"draft":false,"assets":[]}]"#;
        assert_eq!(tag(list, "0.3.0-rc.2").as_deref(), Some("v0.3.0"));
    }

    #[test]
    fn a_build_from_source_is_not_offered_a_prerelease() {
        assert_eq!(tag(SUFFIXED, "0.2.0-dev"), None);
    }

    /// The updater knows the installed app only by the bundle id in its Info.plist.
    #[test]
    fn the_updater_looks_for_the_bundle_id_the_app_ships_with() {
        let plist = include_str!("../../../packaging/macos/Info.plist.in");
        let key = "<key>CFBundleIdentifier</key>";
        let after = &plist[plist.find(key).expect("the plist names its id") + key.len()..];
        let id = after
            .split("<string>")
            .nth(1)
            .and_then(|s| s.split("</string>").next())
            .expect("the id is a string");
        assert_eq!(APP.bundle_id(), id);
    }

    #[test]
    fn a_draft_is_never_offered() {
        let list = r#"[
            {"tag_name":"v0.1.5","prerelease":true,"draft":true,"assets":[]},
            {"tag_name":"v0.1.4","prerelease":true,"draft":false,"assets":[]},
            {"tag_name":"v0.1.3","prerelease":true,"draft":false,"assets":[]}]"#;
        assert_eq!(tag(list, "0.1.3").as_deref(), Some("v0.1.4"));
    }
}
