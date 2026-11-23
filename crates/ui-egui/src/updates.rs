//! Updates, as in the Craft apps: a quiet check at start, Help ▸ Check for Updates, the
//! update prompt and the self-update. craft-update does the work and draws the dialogs.

use std::sync::Arc;

use craft_update::http::Fetched;
use craft_update::updater::{Action, CheckSource, Updater};
use craft_update::{App, CheckResultDialog, CheckResultResponse, Http, Release, version};

/// CraftRadio as its GitHub releases know it.
pub static APP: App = App {
    developer: "RadioTerminal",
    ..App::new(
        "craftradio",
        crate::menus::APP_NAME,
        "Radio station manager",
        "RadioTerminal/craftradio",
    )
};

/// The version of this build. The release workflow writes it into the workspace.
const VERSION: &str = env!("CARGO_PKG_VERSION");

/// True when a copy running `running` follows prereleases: it is one itself (`0.2.0-rc.1`). A
/// stable copy and a build from source follow stable releases only.
fn follows_prereleases(running: &str) -> bool {
    version::parse(running).is_some_and(|v| !version::is_stable(&v) && !version::is_local_build(&v))
}

/// The release in GitHub's release list `json` to offer a prerelease copy running `running`:
/// the newest published release, prerelease or not, when it is newer than the copy.
fn offered_to_prerelease(json: &str, running: &str) -> Result<Option<Release>, String> {
    let newest = craft_update::parse_github_release_list(json, APP.repo, true)?;
    let running = version::parse(running);
    Ok(newest.filter(|release| {
        release
            .version
            .as_ref()
            .zip(running.as_ref())
            .is_some_and(|(newest, running)| version::offers_update(newest, running))
    }))
}

/// The check of a prerelease copy. craft-update's own check offers stable releases only, so a
/// prerelease would never hear of the next one.
fn prerelease_check(running: String) -> CheckSource {
    Arc::new(move || {
        let http = Http::new(&format!("{}/Update", APP.name))?;
        let url = format!(
            "https://api.github.com/repos/{}/releases?per_page=30",
            APP.repo
        );
        match http.get_api(&url, None)? {
            Fetched::Body { text, .. } => offered_to_prerelease(&text, &running),
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
        let mut updater = Updater::new(&APP, VERSION);
        if follows_prereleases(&running) {
            updater = updater.with_source(prerelease_check(running));
        }
        Self {
            updater,
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

    const LIST: &str = r#"[
        {"tag_name":"v0.3.0-rc.2","prerelease":true,"draft":false,"assets":[]},
        {"tag_name":"v0.3.0-rc.1","prerelease":true,"draft":false,"assets":[]},
        {"tag_name":"v0.2.0","prerelease":false,"draft":false,"assets":[]}]"#;

    fn offered(list: &str, running: &str) -> Option<String> {
        offered_to_prerelease(list, running)
            .expect("the list reads")
            .map(|release| release.tag)
    }

    #[test]
    fn only_a_prerelease_copy_follows_prereleases() {
        assert!(follows_prereleases("0.3.0-rc.1"));
        assert!(!follows_prereleases("0.2.0"), "a stable copy stays stable");
        assert!(
            !follows_prereleases("0.3.0-dev"),
            "a build from source is not a published prerelease"
        );
    }

    #[test]
    fn a_prerelease_copy_is_offered_the_next_prerelease() {
        assert_eq!(offered(LIST, "0.3.0-rc.1").as_deref(), Some("v0.3.0-rc.2"));
        assert_eq!(
            offered(LIST, "0.3.0-rc.2"),
            None,
            "the newest has no update"
        );
    }

    #[test]
    fn a_prerelease_copy_is_offered_the_stable_release_after_it() {
        let list = r#"[
            {"tag_name":"v0.3.0","prerelease":false,"draft":false,"assets":[]},
            {"tag_name":"v0.3.0-rc.2","prerelease":true,"draft":false,"assets":[]}]"#;
        assert_eq!(offered(list, "0.3.0-rc.2").as_deref(), Some("v0.3.0"));
    }

    #[test]
    fn a_draft_is_never_offered() {
        let list = r#"[
            {"tag_name":"v0.3.0-rc.3","prerelease":true,"draft":true,"assets":[]},
            {"tag_name":"v0.3.0-rc.2","prerelease":true,"draft":false,"assets":[]}]"#;
        assert_eq!(offered(list, "0.3.0-rc.1").as_deref(), Some("v0.3.0-rc.2"));
    }
}
