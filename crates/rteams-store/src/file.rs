use std::collections::HashMap;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use fs2::FileExt;
use rteams_core::{Command, Event, EventKind, Message, Scope, State, Task};
use serde::{Deserialize, Serialize};

use crate::{Executed, Store, StoreError};

/// One log line: the event plus an optional idempotency key, flattened into one JSON object.
#[derive(Serialize, Deserialize)]
struct Line {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    command_id: Option<String>,
    #[serde(flatten)]
    event: Event,
}

/// Append-only JSONL event log for one team, safe across processes.
pub struct FileStore {
    dir: PathBuf,
    log_path: PathBuf,
    /// `None` for a read-only store: it never locks, appends or repairs.
    lock_file: Option<File>,
    events: Vec<Event>,
    state: State,
    /// command_id -> index into `events`.
    commands: HashMap<String, usize>,
    /// Bytes of the log already folded in (always at a line boundary).
    offset: u64,
}

struct Unlock<'a>(&'a File);
impl Drop for Unlock<'_> {
    fn drop(&mut self) {
        let _ = FileExt::unlock(self.0);
    }
}

fn io(path: &Path) -> impl FnOnce(std::io::Error) -> StoreError + '_ {
    move |source| StoreError::Io {
        path: path.to_path_buf(),
        source,
    }
}

fn valid_team_name(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 64
        && !s.starts_with('.')
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
}

impl FileStore {
    /// Open (creating if needed) the store for `team` under `root` and fold the existing log.
    pub fn open(root: impl AsRef<Path>, team: &str) -> Result<Self, StoreError> {
        if !valid_team_name(team) {
            return Err(StoreError::InvalidTeamName(team.to_string()));
        }
        let dir = root.as_ref().join(team);
        fs::create_dir_all(&dir).map_err(io(&dir))?;
        let log_path = dir.join("events.jsonl");
        let lock_path = dir.join(".lock");
        let lock_file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(&lock_path)
            .map_err(io(&lock_path))?;
        let created = !log_path.exists();
        OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log_path)
            .map_err(io(&log_path))?;
        if created {
            sync_dir(&dir);
        }
        let mut store = FileStore {
            dir,
            log_path,
            lock_file: Some(lock_file),
            events: Vec::new(),
            state: State::default(),
            commands: HashMap::new(),
            offset: 0,
        };
        store.refresh()?;
        Ok(store)
    }

    /// Open an existing team's log for reading only. Creates nothing (no directory, log or
    /// lock file), never repairs a torn tail, and refuses every write with `ReadOnly`.
    /// A missing team is an `Io` error with `ErrorKind::NotFound`.
    pub fn open_read_only(root: impl AsRef<Path>, team: &str) -> Result<Self, StoreError> {
        if !valid_team_name(team) {
            return Err(StoreError::InvalidTeamName(team.to_string()));
        }
        let dir = root.as_ref().join(team);
        let log_path = dir.join("events.jsonl");
        File::open(&log_path).map_err(io(&log_path))?;
        let mut store = FileStore {
            dir,
            log_path,
            lock_file: None,
            events: Vec::new(),
            state: State::default(),
            commands: HashMap::new(),
            offset: 0,
        };
        store.refresh()?;
        Ok(store)
    }

    /// Events folded in so far, without catching up first. With `refresh()` this gives a
    /// consistent view that only moves when the caller says so (e.g. a UI frame).
    pub fn loaded_events(&self) -> &[Event] {
        &self.events
    }

    /// State folded in so far, without catching up first.
    pub fn loaded_state(&self) -> &State {
        &self.state
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Fold in whatever other writers appended since we last looked. No lock needed: an
    /// unterminated tail (a write in flight, or a crash) is simply not consumed yet.
    pub fn refresh(&mut self) -> Result<(), StoreError> {
        self.catch_up(false)
    }

    fn write(
        &mut self,
        actor: &str,
        ts: i64,
        command_id: Option<&str>,
        cmd: Command,
        guard: &dyn Fn(&State) -> Result<(), String>,
    ) -> Result<Executed, StoreError> {
        let Some(lock) = &self.lock_file else {
            return Err(StoreError::ReadOnly);
        };
        let lock_path = self.dir.join(".lock");
        lock.lock_exclusive().map_err(io(&lock_path))?;
        let lock_file = lock.try_clone().map_err(io(&lock_path))?;
        let _guard = Unlock(&lock_file);

        self.catch_up(true)?;

        if let Some(&i) = command_id.and_then(|id| self.commands.get(id)) {
            return Ok(Executed {
                event: self.events[i].clone(),
                replayed: true,
            });
        }

        let kind = self.state.decide(cmd)?;
        guard(&self.state).map_err(StoreError::Forbidden)?;
        let event = Event {
            seq: self.events.len() as u64 + 1,
            ts,
            actor: actor.to_string(),
            kind,
        };
        let line = Line {
            command_id: command_id.map(str::to_string),
            event,
        };
        let mut bytes = serde_json::to_vec(&line).expect("event serializes");
        bytes.push(b'\n');

        let path = &self.log_path;
        let mut f = OpenOptions::new()
            .append(true)
            .open(path)
            .map_err(io(path))?;
        f.write_all(&bytes).map_err(io(path))?;
        f.sync_data().map_err(io(path))?;

        self.offset += bytes.len() as u64;
        self.fold(line);
        if let Some(id) = task_of(&self.events.last().expect("just folded").kind) {
            self.write_snapshot(&id);
        }
        Ok(Executed {
            event: self.events.last().expect("just folded").clone(),
            replayed: false,
        })
    }

    /// Regenerate every task snapshot from the current state (e.g. after copying a log in).
    pub fn rewrite_snapshots(&mut self) -> Result<(), StoreError> {
        if self.lock_file.is_none() {
            return Err(StoreError::ReadOnly);
        }
        self.refresh()?;
        let ids: Vec<String> = self.state.tasks.iter().map(|t| t.id.clone()).collect();
        for id in ids {
            self.snapshot_io(&id)?;
        }
        Ok(())
    }

    /// Best effort: the log is the truth, so a failed snapshot never fails a committed command.
    fn write_snapshot(&self, id: &str) {
        let _ = self.snapshot_io(id);
    }

    /// Write-only, for humans and grep: `tasks/<displayId>.json`. Never read back.
    fn snapshot_io(&self, id: &str) -> Result<(), StoreError> {
        let Some(task) = self.state.task(id) else {
            return Ok(());
        };
        let comments: Vec<&Message> = self
            .state
            .messages
            .iter()
            .filter(|m| matches!(&m.scope, Scope::Task { task } if task == id))
            .collect();
        let snap = Snapshot {
            display_id: display_id(id),
            task,
            comments,
        };
        let dir = self.dir.join("tasks");
        fs::create_dir_all(&dir).map_err(io(&dir))?;
        let name = self.snapshot_name(id);
        let mut bytes = serde_json::to_vec_pretty(&snap).expect("snapshot serializes");
        bytes.push(b'\n');
        atomic_write(&dir.join(format!("{name}.json")), &bytes)
    }

    /// `displayId`, unless another task shares it; then the sanitized full id.
    fn snapshot_name(&self, id: &str) -> String {
        let d = display_id(id);
        let clash = self
            .state
            .tasks
            .iter()
            .any(|t| t.id != id && display_id(&t.id) == d);
        if clash {
            safe_component(id)
        } else {
            d
        }
    }

    fn fold(&mut self, line: Line) {
        if let Some(id) = line.command_id {
            self.commands.insert(id, self.events.len());
        }
        self.state.apply(&line.event);
        self.events.push(line.event);
    }

    /// Read complete lines past `offset`. With `repair` (caller holds the lock), an unterminated
    /// tail left by a crashed writer is truncated away so the next append starts on a clean line.
    fn catch_up(&mut self, repair: bool) -> Result<(), StoreError> {
        let path = self.log_path.clone();
        let mut f = File::open(&path).map_err(io(&path))?;
        f.seek(SeekFrom::Start(self.offset)).map_err(io(&path))?;
        let mut buf = Vec::new();
        f.read_to_end(&mut buf).map_err(io(&path))?;

        let complete = buf.iter().rposition(|&b| b == b'\n').map_or(0, |i| i + 1);
        for raw in buf[..complete]
            .split(|&b| b == b'\n')
            .filter(|l| !l.is_empty())
        {
            let want = self.events.len() as u64 + 1;
            let corrupt = |reason: String| StoreError::Corrupt {
                path: path.clone(),
                line: want,
                reason,
            };
            let line: Line = serde_json::from_slice(raw).map_err(|e| corrupt(e.to_string()))?;
            if line.event.seq != want {
                return Err(corrupt(format!(
                    "expected seq {want}, found {}",
                    line.event.seq
                )));
            }
            self.fold(line);
        }
        self.offset += complete as u64;

        if repair && complete < buf.len() {
            let f = OpenOptions::new()
                .write(true)
                .open(&path)
                .map_err(io(&path))?;
            f.set_len(self.offset).map_err(io(&path))?;
            f.sync_all().map_err(io(&path))?;
        }
        Ok(())
    }
}

impl Store for FileStore {
    /// The id is persisted on the same log line as the event, so dedup and append are atomic.
    fn execute_guarded(
        &mut self,
        actor: &str,
        ts: i64,
        command_id: Option<&str>,
        cmd: Command,
        guard: &dyn Fn(&State) -> Result<(), String>,
    ) -> Result<Executed, StoreError> {
        self.write(actor, ts, command_id, cmd, guard)
    }

    fn events(&mut self) -> Result<&[Event], StoreError> {
        self.refresh()?;
        Ok(&self.events)
    }

    fn state(&mut self) -> Result<&State, StoreError> {
        self.refresh()?;
        Ok(&self.state)
    }
}

/// Persist a newly created file's directory entry. Best effort; a no-op where unsupported.
fn sync_dir(dir: &Path) {
    if let Ok(d) = File::open(dir) {
        let _ = d.sync_all();
    }
}

#[derive(Serialize)]
struct Snapshot<'a> {
    display_id: String,
    #[serde(flatten)]
    task: &'a Task,
    comments: Vec<&'a Message>,
}

fn task_of(kind: &EventKind) -> Option<String> {
    use EventKind::*;
    match kind {
        TaskCreated { task, .. }
        | TaskDependenciesSet { task, .. }
        | TaskAssigned { task, .. }
        | TaskTransitioned { task, .. }
        | TaskBlocked { task, .. }
        | TaskUnblocked { task, .. }
        | TaskArchived { task, .. }
        | TaskCommented { task, .. }
        | ReviewRequested { task, .. }
        | ReviewApproved { task, .. }
        | ReviewChangesRequested { task, .. } => Some(task.clone()),
        TeamCreated { .. }
        | MemberAdded { .. }
        | MessageSent { .. }
        | ProcessRegistered { .. }
        | ProcessStopped { .. } => None,
    }
}

/// First 8 chars of the id, reduced to filename-safe characters.
fn display_id(id: &str) -> String {
    safe_component(&id.chars().take(8).collect::<String>())
}

/// Task ids are caller-supplied strings: never let one pick a path.
fn safe_component(s: &str) -> String {
    let mut out: String = s
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '-' | '_') {
                c
            } else {
                '_'
            }
        })
        .take(64)
        .collect();
    if out.is_empty() {
        out.push('_');
    }
    out
}

/// temp file -> fsync -> rename, retrying the transient sharing errors Windows throws while a
/// reader (antivirus, an open editor) holds the destination.
fn atomic_write(path: &Path, bytes: &[u8]) -> Result<(), StoreError> {
    let tmp = path.with_extension(format!("tmp.{}", std::process::id()));
    let result = (|| {
        let mut f = File::create(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
        drop(f);
        let mut attempt = 0;
        loop {
            match fs::rename(&tmp, path) {
                Ok(()) => return Ok(()),
                Err(e) if attempt < 8 && retryable(&e) => {
                    attempt += 1;
                    std::thread::sleep(std::time::Duration::from_millis(5 << attempt));
                }
                Err(e) => return Err(e),
            }
        }
    })();
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    } else if let Some(dir) = path.parent() {
        sync_dir(dir);
    }
    result.map_err(io(path))
}

fn retryable(e: &std::io::Error) -> bool {
    use std::io::ErrorKind::PermissionDenied;
    // Windows: EPERM/EACCES surface as PermissionDenied; EBUSY/sharing violation (os 32) as Other.
    e.kind() == PermissionDenied || (cfg!(windows) && matches!(e.raw_os_error(), Some(5 | 32)))
}
