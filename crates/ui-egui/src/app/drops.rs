//! Files dropped from the file manager. The view source reads them; the app only routes them.

use super::*;

/// The note about skipped files, or nothing.
pub(super) fn skipped_note(skipped: usize) -> String {
    if skipped > 0 {
        format!(" · skipped {skipped} non-audio files")
    } else {
        String::new()
    }
}

impl RadioApp {
    pub(super) fn add_files_to(&mut self, ctx: &egui::Context, paths: &[PathBuf]) {
        let target = self
            .link
            .file_target
            .clone()
            .or(self.link.last_file_target.clone());
        let Some(target) = target else {
            self.toast(ctx, "drop onto a playlist".to_owned());
            return;
        };
        let paths: Vec<String> = paths.iter().map(|p| p.display().to_string()).collect();
        let imported = match self.source.import_files(paths, "Unsorted") {
            Ok(i) => i,
            Err(e) => {
                self.toast(ctx, e);
                return;
            }
        };
        let note = skipped_note(imported.skipped);
        if imported.ids.is_empty() {
            self.toast(ctx, format!("preview — no audio files to add{note}"));
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
        let result = match target {
            FileTarget::Main(at) => self.insert_items(&imported.ids, at),
            FileTarget::Playlist(name) => {
                let mut placed = 0;
                let mut error = None;
                for id in &imported.ids {
                    match self
                        .source
                        .drop_item(*id, DropTarget::Playlist(name.clone()))
                    {
                        Ok(n) => placed += n,
                        Err(e) => error = Some(e),
                    }
                }
                error.map_or(Ok(placed), Err)
            }
            FileTarget::Editor => {
                self.refresh(true);
                let items: Vec<(String, Duration)> = imported
                    .ids
                    .iter()
                    .filter_map(|id| self.view.library.iter().find(|l| l.id == *id))
                    .map(|l| (l.title.clone(), l.len))
                    .collect();
                let n = items.len();
                if let Some(e) = self.editor.as_mut() {
                    e.add_items(items);
                }
                Ok(n)
            }
            FileTarget::Library => Ok(imported.ids.len()),
        };
        let text = match result {
            Ok(n) => format!("preview — added {n} files to {place} (not saved){note}"),
            Err(e) => format!("{e}{note}"),
        };
        self.toast(ctx, text);
    }
}
