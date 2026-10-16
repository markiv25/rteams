//! task_folders::model: pure folder tree + task assignment operations with invariants
//! (unique ids, no cycles, depth <= MAX_DEPTH, names 1..=120 chars).

use std::collections::{BTreeMap, HashSet};

use serde::{Deserialize, Serialize};

pub const MAX_DEPTH: usize = 8;
pub const MAX_NAME: usize = 120;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Folder {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub parent: Option<String>,
    #[serde(default)]
    pub order: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum FolderError {
    #[error("Use a name with 1-120 characters.")]
    InvalidName,
    #[error("folder not found: {0}")]
    NotFound(String),
    #[error("a folder cannot move into itself or its own subfolder")]
    Cycle,
    #[error("folders can be nested at most {MAX_DEPTH} levels deep")]
    TooDeep,
    #[error("Folder storage is unavailable or invalid. Organization is read only.")]
    ReadOnly,
    #[error("Could not save folder organization. Your last saved structure is preserved.")]
    Save,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct TaskFolders {
    pub version: u32,
    pub folders: Vec<Folder>,
    /// task id -> folder id.
    pub assignments: BTreeMap<String, String>,
}

impl Default for TaskFolders {
    fn default() -> Self {
        TaskFolders {
            version: 1,
            folders: vec![],
            assignments: BTreeMap::new(),
        }
    }
}

/// Trim and validate a folder name (1..=120 chars).
pub fn clean_name(name: &str) -> Result<String, FolderError> {
    let n = name.trim();
    let len = n.chars().count();
    if len == 0 || len > MAX_NAME {
        return Err(FolderError::InvalidName);
    }
    Ok(n.to_string())
}

impl TaskFolders {
    pub fn folder(&self, id: &str) -> Option<&Folder> {
        self.folders.iter().find(|f| f.id == id)
    }

    /// Children of `parent` (None = roots), by `order` then name.
    pub fn children(&self, parent: Option<&str>) -> Vec<&Folder> {
        let mut v: Vec<&Folder> = self
            .folders
            .iter()
            .filter(|f| f.parent.as_deref() == parent)
            .collect();
        v.sort_by_key(|f| (f.order, f.name.to_lowercase()));
        v
    }

    /// Depth of a folder: roots are 1.
    pub fn depth(&self, id: &str) -> usize {
        let mut d = 0;
        let mut cur = Some(id.to_string());
        while let Some(c) = cur {
            d += 1;
            if d > MAX_DEPTH + 64 {
                break;
            }
            cur = self.folder(&c).and_then(|f| f.parent.clone());
        }
        d
    }

    /// Height of the subtree rooted at `id` (a leaf is 1).
    fn height(&self, id: &str) -> usize {
        1 + self
            .children(Some(id))
            .iter()
            .map(|c| self.height(&c.id))
            .max()
            .unwrap_or(0)
    }

    /// `id` and all its descendants.
    pub fn subtree_ids(&self, id: &str) -> HashSet<String> {
        let mut out = HashSet::new();
        let mut stack = vec![id.to_string()];
        while let Some(c) = stack.pop() {
            if out.insert(c.clone()) {
                for ch in self.children(Some(&c)) {
                    stack.push(ch.id.clone());
                }
            }
        }
        out
    }

    fn next_order(&self, parent: Option<&str>) -> u32 {
        self.folders
            .iter()
            .filter(|f| f.parent.as_deref() == parent)
            .map(|f| f.order + 1)
            .max()
            .unwrap_or(0)
    }

    fn new_id(&self) -> String {
        loop {
            let id = format!("f-{}", &uuid::Uuid::new_v4().simple().to_string()[..10]);
            if self.folder(&id).is_none() {
                return id;
            }
        }
    }

    pub fn create(&mut self, name: &str, parent: Option<&str>) -> Result<String, FolderError> {
        let name = clean_name(name)?;
        if let Some(p) = parent {
            if self.folder(p).is_none() {
                return Err(FolderError::NotFound(p.into()));
            }
            if self.depth(p) + 1 > MAX_DEPTH {
                return Err(FolderError::TooDeep);
            }
        }
        let id = self.new_id();
        let order = self.next_order(parent);
        self.folders.push(Folder {
            id: id.clone(),
            name,
            parent: parent.map(String::from),
            order,
        });
        Ok(id)
    }

    pub fn rename(&mut self, id: &str, name: &str) -> Result<(), FolderError> {
        let name = clean_name(name)?;
        let f = self
            .folders
            .iter_mut()
            .find(|f| f.id == id)
            .ok_or_else(|| FolderError::NotFound(id.into()))?;
        f.name = name;
        Ok(())
    }

    /// Delete a folder: subfolders and tasks move to its parent; tasks of a root folder become
    /// unfiled. Returns nothing; no task is ever deleted.
    pub fn delete(&mut self, id: &str) -> Result<(), FolderError> {
        let parent = self
            .folder(id)
            .ok_or_else(|| FolderError::NotFound(id.into()))?
            .parent
            .clone();
        for f in self
            .folders
            .iter_mut()
            .filter(|f| f.parent.as_deref() == Some(id))
        {
            f.parent = parent.clone();
        }
        match &parent {
            Some(p) => {
                for v in self.assignments.values_mut().filter(|v| v.as_str() == id) {
                    *v = p.clone();
                }
            }
            None => self.assignments.retain(|_, v| v != id),
        }
        self.folders.retain(|f| f.id != id);
        Ok(())
    }

    /// Move a folder under `new_parent` (None = root).
    pub fn move_folder(&mut self, id: &str, new_parent: Option<&str>) -> Result<(), FolderError> {
        if self.folder(id).is_none() {
            return Err(FolderError::NotFound(id.into()));
        }
        if let Some(p) = new_parent {
            if self.folder(p).is_none() {
                return Err(FolderError::NotFound(p.into()));
            }
            if self.subtree_ids(id).contains(p) {
                return Err(FolderError::Cycle);
            }
            if self.depth(p) + self.height(id) > MAX_DEPTH {
                return Err(FolderError::TooDeep);
            }
        }
        let order = self.next_order(new_parent);
        let f = self.folders.iter_mut().find(|f| f.id == id).unwrap();
        f.parent = new_parent.map(String::from);
        f.order = order;
        Ok(())
    }

    /// Put a task in a folder, or unfile it with `None`.
    pub fn assign(&mut self, task: &str, folder: Option<&str>) -> Result<(), FolderError> {
        match folder {
            Some(f) => {
                if self.folder(f).is_none() {
                    return Err(FolderError::NotFound(f.into()));
                }
                self.assignments.insert(task.to_string(), f.to_string());
            }
            None => {
                self.assignments.remove(task);
            }
        }
        Ok(())
    }

    pub fn folder_of(&self, task: &str) -> Option<&str> {
        self.assignments.get(task).map(String::as_str)
    }

    /// Task ids filed in `folder` (and its subfolders when `recursive`).
    pub fn tasks_in(&self, folder: &str, recursive: bool) -> HashSet<String> {
        let set = if recursive {
            self.subtree_ids(folder)
        } else {
            HashSet::from([folder.to_string()])
        };
        self.assignments
            .iter()
            .filter(|(_, f)| set.contains(f.as_str()))
            .map(|(t, _)| t.clone())
            .collect()
    }

    /// Remove all organization.
    pub fn reset(&mut self) {
        self.folders.clear();
        self.assignments.clear();
    }

    /// Check structural invariants (used when loading a file).
    pub fn validate(&self) -> Result<(), String> {
        let mut ids = HashSet::new();
        for f in &self.folders {
            if !ids.insert(f.id.as_str()) {
                return Err(format!("duplicate folder id {}", f.id));
            }
            if clean_name(&f.name).is_err() {
                return Err(format!("invalid name for {}", f.id));
            }
        }
        for f in &self.folders {
            if let Some(p) = &f.parent {
                if !ids.contains(p.as_str()) {
                    return Err(format!("missing parent {p}"));
                }
            }
            // Walk up; a cycle or excessive depth never terminates within the bound.
            let mut cur = f.parent.clone();
            let mut d = 1;
            while let Some(c) = cur {
                d += 1;
                if d > MAX_DEPTH {
                    return Err(format!("folder {} is too deep or cyclic", f.id));
                }
                cur = self.folder(&c).and_then(|x| x.parent.clone());
            }
        }
        for v in self.assignments.values() {
            if !ids.contains(v.as_str()) {
                return Err(format!("assignment to missing folder {v}"));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_rename_validate_names() {
        let mut t = TaskFolders::default();
        assert_eq!(t.create("  ", None), Err(FolderError::InvalidName));
        assert_eq!(
            t.create(&"x".repeat(121), None),
            Err(FolderError::InvalidName)
        );
        let a = t.create(" A ", None).unwrap();
        assert_eq!(t.folder(&a).unwrap().name, "A");
        t.rename(&a, "B").unwrap();
        assert_eq!(t.folder(&a).unwrap().name, "B");
        assert!(t.rename("nope", "x").is_err());
    }

    #[test]
    fn depth_limit_and_cycles() {
        let mut t = TaskFolders::default();
        let mut cur = t.create("0", None).unwrap();
        let root = cur.clone();
        for i in 1..MAX_DEPTH {
            cur = t.create(&i.to_string(), Some(&cur)).unwrap();
        }
        assert_eq!(t.create("deep", Some(&cur)), Err(FolderError::TooDeep));
        assert_eq!(t.move_folder(&root, Some(&cur)), Err(FolderError::Cycle));
        let other = t.create("o", None).unwrap();
        // moving the 8-high chain under another root would exceed the depth
        assert_eq!(
            t.move_folder(&root, Some(&other)),
            Err(FolderError::TooDeep)
        );
        t.validate().unwrap();
    }

    #[test]
    fn delete_moves_tasks_and_children_to_parent() {
        let mut t = TaskFolders::default();
        let a = t.create("a", None).unwrap();
        let b = t.create("b", Some(&a)).unwrap();
        let c = t.create("c", Some(&b)).unwrap();
        t.assign("t1", Some(&b)).unwrap();
        t.assign("t2", Some(&c)).unwrap();
        t.delete(&b).unwrap();
        assert_eq!(t.folder(&c).unwrap().parent.as_deref(), Some(a.as_str()));
        assert_eq!(t.folder_of("t1"), Some(a.as_str()));
        assert_eq!(t.folder_of("t2"), Some(c.as_str()));
        t.delete(&a).unwrap();
        assert_eq!(t.folder_of("t1"), None);
        assert_eq!(t.folder(&c).unwrap().parent, None);
        t.validate().unwrap();
    }

    #[test]
    fn tasks_in_recursive_and_reset() {
        let mut t = TaskFolders::default();
        let a = t.create("a", None).unwrap();
        let b = t.create("b", Some(&a)).unwrap();
        t.assign("t1", Some(&a)).unwrap();
        t.assign("t2", Some(&b)).unwrap();
        assert_eq!(t.tasks_in(&a, false).len(), 1);
        assert_eq!(t.tasks_in(&a, true).len(), 2);
        t.assign("t1", None).unwrap();
        assert_eq!(t.tasks_in(&a, true).len(), 1);
        assert!(t.assign("t3", Some("zz")).is_err());
        t.reset();
        assert!(t.folders.is_empty() && t.assignments.is_empty());
    }

    #[test]
    fn validate_catches_bad_files() {
        let mut t = TaskFolders::default();
        t.folders.push(Folder {
            id: "a".into(),
            name: "a".into(),
            parent: Some("b".into()),
            order: 0,
        });
        t.folders.push(Folder {
            id: "b".into(),
            name: "b".into(),
            parent: Some("a".into()),
            order: 0,
        });
        assert!(t.validate().is_err());
    }
}
