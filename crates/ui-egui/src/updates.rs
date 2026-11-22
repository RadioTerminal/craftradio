//! Updates, as in the Craft apps: a quiet check at start, Help ▸ Check for Updates, the
//! update prompt and the self-update. craft-update does the work and draws the dialogs.

use craft_update::updater::{Action, Updater};
use craft_update::{App, CheckResultDialog, CheckResultResponse};

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

/// The updater and the card that tells what a requested check found.
pub(crate) struct Updates {
    updater: Updater,
    result: CheckResultDialog,
}

impl Updates {
    pub(crate) fn new() -> Self {
        Self {
            updater: Updater::new(&APP, VERSION),
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
