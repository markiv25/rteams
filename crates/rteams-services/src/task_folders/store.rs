//! task_folders::store: `<home>/<team>/task-folders.json` persistence. A save error keeps the
//! in-memory state and the last good file; an invalid file opens the store read-only.

use std::path::{Path, PathBuf};

use super::model::{FolderError, TaskFolders};

pub struct FolderStore {
    path: PathBuf,
    pub data: TaskFolders,
    pub read_only: bool,
}

pub fn file_path(home: &Path, team: &str) -> PathBuf {
    home.join(team).join("task-folders.json")
}

impl FolderStore {
    pub fn open(home: &Path, team: &str) -> FolderStore {
        Self::open_path(file_path(home, team))
    }

    pub fn open_path(path: PathBuf) -> FolderStore {
        let (data, read_only) = match std::fs::read(&path) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => (TaskFolders::default(), false),
            Err(_) => (TaskFolders::default(), true),
            Ok(b) => match serde_json::from_slice::<TaskFolders>(&b) {
                Ok(d) if d.validate().is_ok() => (d, false),
                _ => (TaskFolders::default(), true),
            },
        };
        FolderStore {
            path,
            data,
            read_only,
        }
    }

    /// Re-read from disk (picks up CLI / other-process edits). Dropped if read-only.
    pub fn reload(&mut self) {
        *self = Self::open_path(self.path.clone());
    }

    /// Apply `f` to a copy, persist it, and only then adopt it.
    pub fn mutate<R>(
        &mut self,
        f: impl FnOnce(&mut TaskFolders) -> Result<R, FolderError>,
    ) -> Result<R, FolderError> {
        if self.read_only {
            return Err(FolderError::ReadOnly);
        }
        let mut next = self.data.clone();
        let r = f(&mut next)?;
        let bytes = serde_json::to_vec_pretty(&next).map_err(|_| FolderError::Save)?;
        crate::atomic::write(&self.path, &bytes).map_err(|_| FolderError::Save)?;
        self.data = next;
        Ok(r)
    }

    pub fn create(&mut self, name: &str, parent: Option<&str>) -> Result<String, FolderError> {
        self.mutate(|d| d.create(name, parent))
    }
    pub fn rename(&mut self, id: &str, name: &str) -> Result<(), FolderError> {
        self.mutate(|d| d.rename(id, name))
    }
    pub fn delete(&mut self, id: &str) -> Result<(), FolderError> {
        self.mutate(|d| d.delete(id))
    }
    pub fn move_folder(&mut self, id: &str, parent: Option<&str>) -> Result<(), FolderError> {
        self.mutate(|d| d.move_folder(id, parent))
    }
    pub fn assign(&mut self, task: &str, folder: Option<&str>) -> Result<(), FolderError> {
        self.mutate(|d| d.assign(task, folder))
    }
    pub fn reset(&mut self) -> Result<(), FolderError> {
        self.mutate(|d| {
            d.reset();
            Ok(())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn persists_and_reloads() {
        let dir = tempfile::tempdir().unwrap();
        let mut s = FolderStore::open(dir.path(), "t");
        assert!(!s.read_only);
        let a = s.create("A", None).unwrap();
        s.assign("task1", Some(&a)).unwrap();
        let s2 = FolderStore::open(dir.path(), "t");
        assert_eq!(s2.data.folder_of("task1"), Some(a.as_str()));
        assert_eq!(s2.data.version, 1);
    }

    #[test]
    fn invalid_file_is_read_only_and_untouched() {
        let dir = tempfile::tempdir().unwrap();
        let p = file_path(dir.path(), "t");
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, "{ not json").unwrap();
        let mut s = FolderStore::open(dir.path(), "t");
        assert!(s.read_only);
        assert_eq!(s.create("A", None), Err(FolderError::ReadOnly));
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "{ not json");
    }

    #[test]
    fn failed_op_keeps_state() {
        let dir = tempfile::tempdir().unwrap();
        let mut s = FolderStore::open(dir.path(), "t");
        s.create("A", None).unwrap();
        assert!(s.create("", None).is_err());
        assert_eq!(s.data.folders.len(), 1);
    }
}
