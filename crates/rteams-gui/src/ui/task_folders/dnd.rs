//! task_folders::dnd: drag payload and drop handling (egui DragAndDrop).

use eframe::egui::{self, Response};
use rteams_services::task_folders::{FolderError, FolderStore};

use crate::theme;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DragItem {
    Task(String),
    Folder(String),
}

/// Apply a drop on `target` (None = root / unfiled). Folders cannot be dropped on themselves
/// (the model rejects cycles).
pub fn apply_drop(
    store: &mut FolderStore,
    item: &DragItem,
    target: Option<&str>,
) -> Result<(), FolderError> {
    match item {
        DragItem::Task(t) => store.assign(t, target),
        DragItem::Folder(f) => store.move_folder(f, target),
    }
}

/// Make `resp` start dragging a task. The board can call this on its task cards (the card must
/// be allocated with `Sense::click_and_drag()` or `Sense::drag()`).
#[cfg_attr(not(test), allow(dead_code))] // called by the board when dragging tasks into folders
pub fn make_task_draggable(resp: &Response, task_id: &str) {
    if resp.drag_started() {
        resp.dnd_set_drag_payload(DragItem::Task(task_id.to_string()));
    }
}

/// While a drag hovers `resp`, outline it; returns the payload on release.
pub fn drop_target(ui: &egui::Ui, resp: &Response) -> Option<DragItem> {
    if resp.dnd_hover_payload::<DragItem>().is_some() {
        ui.painter().rect_stroke(
            resp.rect.expand(1.0),
            4.0,
            egui::Stroke::new(1.5, theme::ACCENT),
            egui::StrokeKind::Outside,
        );
    }
    resp.dnd_release_payload::<DragItem>().map(|p| (*p).clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drops_assign_and_move() {
        let dir = tempfile::tempdir().unwrap();
        let mut s = FolderStore::open(dir.path(), "t");
        let a = s.create("a", None).unwrap();
        let b = s.create("b", None).unwrap();
        apply_drop(&mut s, &DragItem::Task("t1".into()), Some(&a)).unwrap();
        assert_eq!(s.data.folder_of("t1"), Some(a.as_str()));
        apply_drop(&mut s, &DragItem::Task("t1".into()), None).unwrap();
        assert_eq!(s.data.folder_of("t1"), None);
        apply_drop(&mut s, &DragItem::Folder(b.clone()), Some(&a)).unwrap();
        assert_eq!(
            s.data.folder(&b).unwrap().parent.as_deref(),
            Some(a.as_str())
        );
        assert_eq!(
            apply_drop(&mut s, &DragItem::Folder(a.clone()), Some(&b)),
            Err(FolderError::Cycle)
        );
    }

    #[test]
    fn make_task_draggable_is_a_noop_without_a_drag() {
        let ctx = egui::Context::default();
        crate::ui::close_guard::headless(&ctx, |ui| {
            let resp = ui.allocate_response(egui::vec2(10.0, 10.0), egui::Sense::drag());
            make_task_draggable(&resp, "t1");
            assert!(resp.dnd_hover_payload::<DragItem>().is_none());
        });
    }
}
