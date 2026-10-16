//! task_folders: user folders grouping kanban tasks (nested, persisted per team).

pub mod model;
pub mod store;

pub use model::{Folder, FolderError, TaskFolders, MAX_DEPTH};
pub use store::FolderStore;
