//! What the org map and inspector show: team summaries read from each team's log plus recent
//! run files, joined with the persisted `OrgMap`.

use std::collections::BTreeMap;
use std::path::Path;
use std::time::{Duration, SystemTime};

use rteams_core::{Event, State, Status};

use super::model::{Group, Org, OrgMap, Relation};

/// A run file touched within this window counts as "running".
pub const RUNNING_WINDOW: Duration = Duration::from_secs(90);

/// Team directories under `home` (those holding an event log), sorted.
pub fn team_dirs(home: &Path) -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir(home)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.path().join("events.jsonl").is_file())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    v.sort();
    v
}

/// Fold a team's log. Unparsable lines (a torn tail) are skipped.
pub fn read_log(home: &Path, team: &str) -> Option<(State, Vec<Event>)> {
    let text = std::fs::read_to_string(home.join(team).join("events.jsonl")).ok()?;
    let events: Vec<Event> = text
        .lines()
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect();
    Some((State::from_events(&events), events))
}

/// Members with a run file modified within `window` of `now`.
pub fn running_members(home: &Path, team: &str, now: SystemTime, window: Duration) -> Vec<String> {
    let mut out = Vec::new();
    for m in std::fs::read_dir(home.join(team).join("runs"))
        .into_iter()
        .flatten()
        .flatten()
    {
        let fresh = std::fs::read_dir(m.path())
            .into_iter()
            .flatten()
            .flatten()
            .any(|f| {
                f.metadata()
                    .and_then(|md| md.modified())
                    .ok()
                    .and_then(|t| now.duration_since(t).ok())
                    .is_some_and(|age| age <= window)
            });
        if fresh {
            out.push(m.file_name().to_string_lossy().into_owned());
        }
    }
    out.sort();
    out
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TeamSummary {
    pub dir: String,
    pub name: String,
    pub members: usize,
    pub leads: Vec<String>,
    pub open_tasks: usize,
    pub in_progress: usize,
    pub done_tasks: usize,
    pub deleted: bool,
    pub running: bool,
    pub running_members: Vec<String>,
}

/// Summary of one team; `None` when it has no readable log.
pub fn team_summary(home: &Path, team: &str, now: SystemTime) -> Option<TeamSummary> {
    let (state, _) = read_log(home, team)?;
    let live = state.tasks.iter().filter(|t| !t.archived);
    let done = live
        .clone()
        .filter(|t| matches!(t.status, Status::Done | Status::Approved))
        .count();
    let in_progress = live
        .clone()
        .filter(|t| t.status == Status::InProgress)
        .count();
    let total = live.count();
    let t = state.team.as_ref();
    let active = |m: &&rteams_core::Member| !m.removed;
    let running_members = running_members(home, team, now, RUNNING_WINDOW);
    Some(TeamSummary {
        dir: team.into(),
        name: t.map_or_else(|| team.to_string(), |t| t.name.clone()),
        members: t.map_or(0, |t| t.members.iter().filter(active).count()),
        leads: t.map_or_else(Vec::new, |t| {
            t.members
                .iter()
                .filter(active)
                .filter(|m| m.role == "lead")
                .map(|m| m.name.clone())
                .collect()
        }),
        open_tasks: total - done,
        in_progress,
        done_tasks: done,
        deleted: t.is_some_and(|t| t.deleted),
        running: !running_members.is_empty(),
        running_members,
    })
}

/// Summaries of every team under `home`, keyed by directory name.
pub fn scan_summaries(home: &Path, now: SystemTime) -> BTreeMap<String, TeamSummary> {
    team_dirs(home)
        .into_iter()
        .filter_map(|d| team_summary(home, &d, now).map(|s| (d, s)))
        .collect()
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Totals {
    pub teams: usize,
    pub members: usize,
    pub open_tasks: usize,
    pub done_tasks: usize,
    pub running: usize,
}

impl Totals {
    fn add(&mut self, s: &TeamSummary) {
        self.teams += 1;
        self.members += s.members;
        self.open_tasks += s.open_tasks;
        self.done_tasks += s.done_tasks;
        self.running += usize::from(s.running);
    }
    fn merge(&mut self, o: &Totals) {
        self.teams += o.teams;
        self.members += o.members;
        self.open_tasks += o.open_tasks;
        self.done_tasks += o.done_tasks;
        self.running += o.running;
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct GroupNode {
    pub group: Group,
    pub teams: Vec<TeamSummary>,
    pub totals: Totals,
}

#[derive(Debug, Clone, PartialEq)]
pub struct OrgNode {
    pub org: Org,
    pub children: Vec<OrgNode>,
    pub groups: Vec<GroupNode>,
    /// Teams placed directly in the org.
    pub teams: Vec<TeamSummary>,
    /// This org and everything below it.
    pub totals: Totals,
}

/// A relation edge with its runtime cross-team message weight.
#[derive(Debug, Clone, PartialEq)]
pub struct RelationEdge {
    pub relation: Relation,
    pub weight: usize,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct OrgView {
    pub roots: Vec<OrgNode>,
    /// Teams under the home that no org places (and relations to them are still listed).
    pub unplaced: Vec<TeamSummary>,
    pub relations: Vec<RelationEdge>,
    /// Runtime edges `(from, to, count)` from cross-team messages.
    pub runtime: Vec<(String, String, usize)>,
    /// Placed team names that no longer exist on disk.
    pub missing: Vec<String>,
}

fn node(map: &OrgMap, org: &Org, teams: &BTreeMap<String, TeamSummary>, depth: usize) -> OrgNode {
    let pick = |names: &[String]| -> Vec<TeamSummary> {
        names.iter().filter_map(|n| teams.get(n).cloned()).collect()
    };
    let groups: Vec<GroupNode> = org
        .groups
        .iter()
        .map(|g| {
            let ts = pick(&g.teams);
            let mut totals = Totals::default();
            ts.iter().for_each(|t| totals.add(t));
            GroupNode {
                group: g.clone(),
                teams: ts,
                totals,
            }
        })
        .collect();
    let direct = pick(&org.teams);
    // Depth guard: a hand-edited file with a parent cycle must not recurse forever.
    let children: Vec<OrgNode> = if depth > 64 {
        Vec::new()
    } else {
        map.orgs
            .iter()
            .filter(|o| o.parent.as_deref() == Some(&org.id))
            .map(|o| node(map, o, teams, depth + 1))
            .collect()
    };
    let mut totals = Totals::default();
    direct.iter().for_each(|t| totals.add(t));
    groups.iter().for_each(|g| totals.merge(&g.totals));
    children.iter().for_each(|c| totals.merge(&c.totals));
    OrgNode {
        org: org.clone(),
        children,
        groups,
        teams: direct,
        totals,
    }
}

impl OrgView {
    pub fn build(
        map: &OrgMap,
        teams: &BTreeMap<String, TeamSummary>,
        counts: &BTreeMap<(String, String), usize>,
    ) -> OrgView {
        let ids: Vec<&str> = map.orgs.iter().map(|o| o.id.as_str()).collect();
        let roots = map
            .orgs
            .iter()
            .filter(|o| o.parent.as_deref().is_none_or(|p| !ids.contains(&p)))
            .map(|o| node(map, o, teams, 0))
            .collect();
        let unplaced = teams
            .values()
            .filter(|t| map.placement(&t.dir).is_none())
            .cloned()
            .collect();
        let mut missing: Vec<String> = map
            .orgs
            .iter()
            .flat_map(|o| {
                o.teams
                    .iter()
                    .chain(o.groups.iter().flat_map(|g| g.teams.iter()))
            })
            .filter(|t| !teams.contains_key(*t))
            .cloned()
            .collect();
        missing.sort();
        missing.dedup();
        let weight = |a: &str, b: &str| {
            counts
                .get(&(a.to_string(), b.to_string()))
                .copied()
                .unwrap_or(0)
                + counts
                    .get(&(b.to_string(), a.to_string()))
                    .copied()
                    .unwrap_or(0)
        };
        OrgView {
            roots,
            unplaced,
            relations: map
                .relations
                .iter()
                .map(|r| RelationEdge {
                    relation: r.clone(),
                    weight: weight(&r.source, &r.target),
                })
                .collect(),
            runtime: counts
                .iter()
                .map(|((a, b), n)| (a.clone(), b.clone(), *n))
                .collect(),
            missing,
        }
    }

    /// Load everything from disk.
    pub fn load(home: &Path, map: &OrgMap) -> OrgView {
        let teams = scan_summaries(home, SystemTime::now());
        OrgView::build(map, &teams, &super::cross_team::message_counts(home))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::orgs::model::RelationKind;

    #[test]
    fn view_without_disk() {
        let mut map = OrgMap::default();
        let a = map.create_org("A", None).unwrap();
        let b = map.create_org("B", Some(&a)).unwrap();
        map.place_team("x", &a, None).unwrap();
        map.place_team("gone", &b, None).unwrap();
        map.add_relation("x", "y", RelationKind::Delegates).unwrap();
        let sum = |d: &str, run: bool| TeamSummary {
            dir: d.into(),
            members: 2,
            open_tasks: 1,
            running: run,
            ..Default::default()
        };
        let teams: BTreeMap<_, _> = [("x", true), ("y", false)]
            .into_iter()
            .map(|(d, r)| (d.to_string(), sum(d, r)))
            .collect();
        let counts = BTreeMap::from([(("y".to_string(), "x".to_string()), 3)]);
        let v = OrgView::build(&map, &teams, &counts);
        assert_eq!(v.roots.len(), 1);
        assert_eq!(v.roots[0].children.len(), 1);
        assert_eq!(v.roots[0].totals.teams, 1);
        assert_eq!(v.roots[0].totals.running, 1);
        assert_eq!(
            v.unplaced
                .iter()
                .map(|t| t.dir.as_str())
                .collect::<Vec<_>>(),
            ["y"]
        );
        assert_eq!(v.missing, ["gone"]);
        assert_eq!(v.relations[0].weight, 3);
    }

    #[test]
    fn parent_cycle_in_file_does_not_hang() {
        let mut map = OrgMap::default();
        let a = map.create_org("A", None).unwrap();
        let b = map.create_org("B", Some(&a)).unwrap();
        map.orgs[0].parent = Some(b);
        let v = OrgView::build(&map, &BTreeMap::new(), &BTreeMap::new());
        assert!(
            v.roots.is_empty(),
            "a pure cycle has no root; it must simply not hang"
        );
    }

    #[test]
    fn summaries_from_logs_and_run_files() {
        let home = tempfile::tempdir().unwrap();
        let ev = |seq: u64, kind: serde_json::Value| {
            let mut v = serde_json::json!({"seq": seq, "ts": seq, "actor": "boss"});
            v.as_object_mut()
                .unwrap()
                .extend(kind.as_object().unwrap().clone());
            v.to_string()
        };
        let dir = home.path().join("t");
        std::fs::create_dir_all(dir.join("runs/ada")).unwrap();
        let lines = [
            ev(1, serde_json::json!({"type": "team_created", "name": "t"})),
            ev(
                2,
                serde_json::json!({"type": "member_added", "name": "boss", "role": "lead"}),
            ),
            ev(
                3,
                serde_json::json!({"type": "member_added", "name": "ada", "role": "dev"}),
            ),
        ];
        std::fs::write(dir.join("events.jsonl"), lines.join("\n") + "\n{torn").unwrap();
        std::fs::write(dir.join("runs/ada/1.jsonl"), "{}").unwrap();
        let now = SystemTime::now();
        let s = team_summary(home.path(), "t", now).expect("summary");
        assert_eq!(s.members, 2);
        assert_eq!(s.leads, ["boss"]);
        assert!(s.running);
        assert_eq!(s.running_members, ["ada"]);
        let later = now + Duration::from_secs(3600);
        assert!(!team_summary(home.path(), "t", later).unwrap().running);
        assert!(team_summary(home.path(), "nope", now).is_none());
    }
}
