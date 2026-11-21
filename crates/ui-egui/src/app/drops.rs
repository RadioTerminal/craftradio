//! Files dropped from the file manager.

use super::*;

impl RadioApp {
    pub(super) fn add_files_to(&mut self, ctx: &egui::Context, paths: &[PathBuf]) {
        let (audio, skipped) = folders::expand_dropped(paths);
        let skipped_note = if skipped > 0 {
            format!(" · skipped {skipped} non-audio files")
        } else {
            String::new()
        };
        let target = self
            .link
            .file_target
            .clone()
            .or(self.link.last_file_target.clone());
        let Some(target) = target else {
            self.toast(ctx, format!("drop onto a playlist{skipped_note}"));
            return;
        };
        let n = audio.len();
        if n == 0 {
            self.toast(
                ctx,
                format!("preview — no audio files to add{skipped_note}"),
            );
            return;
        }
        let place = match &target {
            FileTarget::Main(_) => self.view.main_playlist.clone(),
            FileTarget::Playlist(name) => name.clone(),
            FileTarget::Editor => self
                .editor
                .as_ref()
                .map_or_else(|| "the editor".to_owned(), |e| e.name().to_owned()),
            FileTarget::Library => "the Library".to_owned(),
        };
        let ids = self.source.import_files(audio.clone(), "Unsorted");
        let result = match target {
            FileTarget::Main(at) => self.insert_items(&ids, at).map(|_| String::new()),
            FileTarget::Playlist(name) => {
                for id in &ids {
                    self.source
                        .drop_item(*id, DropTarget::Playlist(name.clone()));
                }
                Ok(String::new())
            }
            FileTarget::Editor => {
                if let Some(e) = self.editor.as_mut() {
                    e.add_files(&audio);
                }
                Ok(String::new())
            }
            FileTarget::Library => Ok(String::new()),
        };
        let text = match result {
            Ok(_) => format!("preview — added {n} files to {place} (not saved){skipped_note}"),
            Err(e) => format!("{e}{skipped_note}"),
        };
        self.toast(ctx, text);
    }
}
