//! Structure operations on an `OrgMap`. Every op validates first and leaves the map untouched
//! on error.

use super::ids::{normalize_id, unique_id};
use super::model::{Group, Org, OrgMap, Relation, RelationKind, DEFAULT_COLOR};

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum OrgError {
    #[error("unknown organization {0}")]
    NoOrg(String),
    #[error("unknown group {0}")]
    NoGroup(String),
    #[error("unknown relation {0}")]
    NoRelation(String),
    #[error("name must not be empty")]
    EmptyName,
    #[error("that would make {0} its own ancestor")]
    Cycle(String),
    #[error("a team cannot relate to itself")]
    SelfRelation,
    #[error("that relation already exists")]
    Duplicate,
}

fn clean(name: &str) -> Result<String, OrgError> {
    let n = name.trim();
    if n.is_empty() {
        Err(OrgError::EmptyName)
    } else {
        Ok(n.to_string())
    }
}

impl OrgMap {
    fn org_mut(&mut self, id: &str) -> Result<&mut Org, OrgError> {
        self.orgs
            .iter_mut()
            .find(|o| o.id == id)
            .ok_or_else(|| OrgError::NoOrg(id.into()))
    }

    /// Create an organization (optionally under `parent`); returns its id.
    pub fn create_org(&mut self, name: &str, parent: Option<&str>) -> Result<String, OrgError> {
        let name = clean(name)?;
        if let Some(p) = parent {
            self.org(p).ok_or_else(|| OrgError::NoOrg(p.into()))?;
        }
        let id = unique_id(&normalize_id(&name), |i| self.org(i).is_some());
        self.orgs.push(Org {
            id: id.clone(),
            name,
            color: DEFAULT_COLOR.into(),
            description: String::new(),
            parent: parent.map(str::to_string),
            groups: Vec::new(),
            teams: Vec::new(),
            extra: Default::default(),
        });
        Ok(id)
    }

    pub fn rename_org(&mut self, id: &str, name: &str) -> Result<(), OrgError> {
        let name = clean(name)?;
        self.org_mut(id)?.name = name;
        Ok(())
    }

    pub fn set_org_color(&mut self, id: &str, color: &str) -> Result<(), OrgError> {
        self.org_mut(id)?.color = color.to_string();
        Ok(())
    }

    pub fn set_org_description(&mut self, id: &str, text: &str) -> Result<(), OrgError> {
        self.org_mut(id)?.description = text.to_string();
        Ok(())
    }

    /// Ids of `id` and everything below it.
    pub fn subtree(&self, id: &str) -> Vec<String> {
        let mut out = vec![id.to_string()];
        let mut i = 0;
        while i < out.len() {
            let cur = out[i].clone();
            let kids: Vec<String> = self
                .orgs
                .iter()
                .filter(|o| o.parent.as_deref() == Some(&cur) && !out.contains(&o.id))
                .map(|o| o.id.clone())
                .collect();
            out.extend(kids);
            i += 1;
        }
        out
    }

    /// Re-parent `id` (`None` = top level). Rejects cycles.
    pub fn set_parent(&mut self, id: &str, parent: Option<&str>) -> Result<(), OrgError> {
        self.org(id).ok_or_else(|| OrgError::NoOrg(id.into()))?;
        if let Some(p) = parent {
            self.org(p).ok_or_else(|| OrgError::NoOrg(p.into()))?;
            if self.subtree(id).iter().any(|s| s == p) {
                return Err(OrgError::Cycle(id.into()));
            }
        }
        self.org_mut(id)?.parent = parent.map(str::to_string);
        Ok(())
    }

    /// Delete an organization. Its child orgs move up to its parent; its teams become
    /// unplaced; relations (between teams) are untouched.
    pub fn delete_org(&mut self, id: &str) -> Result<(), OrgError> {
        let parent = self
            .org(id)
            .ok_or_else(|| OrgError::NoOrg(id.into()))?
            .parent
            .clone();
        for o in &mut self.orgs {
            if o.parent.as_deref() == Some(id) {
                o.parent = parent.clone();
            }
        }
        self.orgs.retain(|o| o.id != id);
        Ok(())
    }

    pub fn create_group(&mut self, org: &str, name: &str) -> Result<String, OrgError> {
        let name = clean(name)?;
        let o = self.org_mut(org)?;
        let id = unique_id(&normalize_id(&name), |i| o.groups.iter().any(|g| g.id == i));
        o.groups.push(Group {
            id: id.clone(),
            name,
            color: o.color.clone(),
            description: String::new(),
            teams: Vec::new(),
            extra: Default::default(),
        });
        Ok(id)
    }

    pub fn rename_group(&mut self, org: &str, group: &str, name: &str) -> Result<(), OrgError> {
        let name = clean(name)?;
        let g = self
            .org_mut(org)?
            .groups
            .iter_mut()
            .find(|g| g.id == group)
            .ok_or_else(|| OrgError::NoGroup(group.into()))?;
        g.name = name;
        Ok(())
    }

    /// Delete a group; its teams stay in the organization, ungrouped.
    pub fn delete_group(&mut self, org: &str, group: &str) -> Result<(), OrgError> {
        let o = self.org_mut(org)?;
        let i = o
            .groups
            .iter()
            .position(|g| g.id == group)
            .ok_or_else(|| OrgError::NoGroup(group.into()))?;
        let g = o.groups.remove(i);
        o.teams.extend(g.teams);
        Ok(())
    }

    /// Take `team` out of wherever it is placed. Returns whether it was placed.
    pub fn remove_team(&mut self, team: &str) -> bool {
        let mut found = false;
        for o in &mut self.orgs {
            let n = o.teams.len();
            o.teams.retain(|t| t != team);
            found |= n != o.teams.len();
            for g in &mut o.groups {
                let n = g.teams.len();
                g.teams.retain(|t| t != team);
                found |= n != g.teams.len();
            }
        }
        found
    }

    /// Place `team` in `org` (inside `group` if given), moving it if already placed.
    pub fn place_team(
        &mut self,
        team: &str,
        org: &str,
        group: Option<&str>,
    ) -> Result<(), OrgError> {
        let o = self.org(org).ok_or_else(|| OrgError::NoOrg(org.into()))?;
        if let Some(g) = group {
            if !o.groups.iter().any(|x| x.id == g) {
                return Err(OrgError::NoGroup(g.into()));
            }
        }
        self.remove_team(team);
        let o = self.org_mut(org)?;
        match group {
            None => o.teams.push(team.to_string()),
            Some(g) => o
                .groups
                .iter_mut()
                .find(|x| x.id == g)
                .expect("checked")
                .teams
                .push(team.to_string()),
        }
        Ok(())
    }

    /// Add a manual relation between two teams; `ReportsTo` lines must stay acyclic.
    pub fn add_relation(
        &mut self,
        source: &str,
        target: &str,
        kind: RelationKind,
    ) -> Result<String, OrgError> {
        if source == target {
            return Err(OrgError::SelfRelation);
        }
        if self
            .relations
            .iter()
            .any(|r| r.source == source && r.target == target && r.kind == kind)
        {
            return Err(OrgError::Duplicate);
        }
        if kind == RelationKind::ReportsTo && self.reports_chain(target).iter().any(|t| t == source)
        {
            return Err(OrgError::Cycle(source.into()));
        }
        let base = format!(
            "{}-{}-{}",
            normalize_id(source),
            kind_slug(kind),
            normalize_id(target)
        );
        let id = unique_id(&base, |i| self.relations.iter().any(|r| r.id == i));
        self.relations.push(Relation {
            id: id.clone(),
            source: source.into(),
            target: target.into(),
            kind,
            extra: Default::default(),
        });
        Ok(id)
    }

    pub fn remove_relation(&mut self, id: &str) -> Result<(), OrgError> {
        let n = self.relations.len();
        self.relations.retain(|r| r.id != id);
        if n == self.relations.len() {
            Err(OrgError::NoRelation(id.into()))
        } else {
            Ok(())
        }
    }

    /// `team` plus every team it (transitively) reports to.
    pub fn reports_chain(&self, team: &str) -> Vec<String> {
        let mut out = vec![team.to_string()];
        let mut i = 0;
        while i < out.len() {
            let cur = out[i].clone();
            for r in &self.relations {
                if r.kind == RelationKind::ReportsTo && r.source == cur && !out.contains(&r.target)
                {
                    out.push(r.target.clone());
                }
            }
            i += 1;
        }
        out
    }
}

/// One edit a UI can request; `OrgMap::apply` runs it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Edit {
    CreateOrg {
        name: String,
        parent: Option<String>,
    },
    RenameOrg {
        id: String,
        name: String,
    },
    SetColor {
        id: String,
        color: String,
    },
    SetDescription {
        id: String,
        text: String,
    },
    SetParent {
        id: String,
        parent: Option<String>,
    },
    DeleteOrg {
        id: String,
    },
    CreateGroup {
        org: String,
        name: String,
    },
    RenameGroup {
        org: String,
        group: String,
        name: String,
    },
    DeleteGroup {
        org: String,
        group: String,
    },
    PlaceTeam {
        team: String,
        org: String,
        group: Option<String>,
    },
    RemoveTeam {
        team: String,
    },
    AddRelation {
        source: String,
        target: String,
        kind: RelationKind,
    },
    RemoveRelation {
        id: String,
    },
}

impl OrgMap {
    /// Run an edit; returns the new id for the `Create*` / `AddRelation` edits.
    pub fn apply(&mut self, e: Edit) -> Result<Option<String>, OrgError> {
        Ok(match e {
            Edit::CreateOrg { name, parent } => Some(self.create_org(&name, parent.as_deref())?),
            Edit::RenameOrg { id, name } => self.rename_org(&id, &name).map(|_| None)?,
            Edit::SetColor { id, color } => self.set_org_color(&id, &color).map(|_| None)?,
            Edit::SetDescription { id, text } => {
                self.set_org_description(&id, &text).map(|_| None)?
            }
            Edit::SetParent { id, parent } => {
                self.set_parent(&id, parent.as_deref()).map(|_| None)?
            }
            Edit::DeleteOrg { id } => self.delete_org(&id).map(|_| None)?,
            Edit::CreateGroup { org, name } => Some(self.create_group(&org, &name)?),
            Edit::RenameGroup { org, group, name } => {
                self.rename_group(&org, &group, &name).map(|_| None)?
            }
            Edit::DeleteGroup { org, group } => self.delete_group(&org, &group).map(|_| None)?,
            Edit::PlaceTeam { team, org, group } => self
                .place_team(&team, &org, group.as_deref())
                .map(|_| None)?,
            Edit::RemoveTeam { team } => {
                self.remove_team(&team);
                None
            }
            Edit::AddRelation {
                source,
                target,
                kind,
            } => Some(self.add_relation(&source, &target, kind)?),
            Edit::RemoveRelation { id } => self.remove_relation(&id).map(|_| None)?,
        })
    }
}

fn kind_slug(k: RelationKind) -> &'static str {
    match k {
        RelationKind::DependsOn => "depends-on",
        RelationKind::Delegates => "delegates",
        RelationKind::Observes => "observes",
        RelationKind::ReportsTo => "reports-to",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> (OrgMap, String, String) {
        let mut m = OrgMap::default();
        let a = m.create_org("Acme", None).unwrap();
        let b = m.create_org("Acme", Some(&a)).unwrap();
        (m, a, b)
    }

    #[test]
    fn create_rename_delete() {
        let (mut m, a, b) = sample();
        assert_eq!((a.as_str(), b.as_str()), ("acme", "acme-2"));
        assert_eq!(m.create_org("  ", None), Err(OrgError::EmptyName));
        assert_eq!(
            m.create_org("x", Some("nope")),
            Err(OrgError::NoOrg("nope".into()))
        );
        m.rename_org(&b, "Platform").unwrap();
        let c = m.create_org("Infra", Some(&b)).unwrap();
        m.delete_org(&b).unwrap();
        assert_eq!(m.org(&c).unwrap().parent.as_deref(), Some("acme"));
        assert!(m.delete_org(&b).is_err());
    }

    #[test]
    fn parent_cycles_are_rejected() {
        let (mut m, a, b) = sample();
        let c = m.create_org("C", Some(&b)).unwrap();
        assert_eq!(m.set_parent(&a, Some(&c)), Err(OrgError::Cycle(a.clone())));
        assert_eq!(m.set_parent(&a, Some(&a)), Err(OrgError::Cycle(a.clone())));
        assert_eq!(m.org(&a).unwrap().parent, None, "unchanged on error");
        m.set_parent(&c, None).unwrap();
        m.set_parent(&a, Some(&c)).unwrap();
        assert_eq!(m.subtree(&c).len(), 3);
    }

    #[test]
    fn teams_move_between_places() {
        let (mut m, a, b) = sample();
        let g = m.create_group(&a, "Squad One").unwrap();
        m.place_team("t", &a, None).unwrap();
        assert_eq!(m.placement("t"), Some(("acme", None)));
        m.place_team("t", &a, Some(&g)).unwrap();
        m.place_team("t", &b, None).unwrap();
        assert_eq!(m.placement("t"), Some(("acme-2", None)));
        assert!(m.org(&a).unwrap().groups[0].teams.is_empty());
        assert_eq!(
            m.place_team("t", &a, Some("zz")),
            Err(OrgError::NoGroup("zz".into()))
        );
        m.place_team("u", &a, Some(&g)).unwrap();
        m.delete_group(&a, &g).unwrap();
        assert_eq!(m.placement("u"), Some(("acme", None)));
        assert!(m.remove_team("u"));
        assert!(!m.remove_team("u"));
    }

    #[test]
    fn apply_edits() {
        let mut m = OrgMap::default();
        let id = m
            .apply(Edit::CreateOrg {
                name: "X".into(),
                parent: None,
            })
            .unwrap()
            .unwrap();
        m.apply(Edit::SetColor {
            id: id.clone(),
            color: "#112233".into(),
        })
        .unwrap();
        assert_eq!(m.org(&id).unwrap().color, "#112233");
        assert!(m.apply(Edit::DeleteOrg { id: "nope".into() }).is_err());
    }

    #[test]
    fn reporting_lines_are_acyclic() {
        let mut m = OrgMap::default();
        m.add_relation("a", "b", RelationKind::ReportsTo).unwrap();
        m.add_relation("b", "c", RelationKind::ReportsTo).unwrap();
        assert_eq!(
            m.add_relation("c", "a", RelationKind::ReportsTo),
            Err(OrgError::Cycle("c".into()))
        );
        // Non-hierarchical kinds may loop.
        m.add_relation("c", "a", RelationKind::Observes).unwrap();
        assert_eq!(
            m.add_relation("a", "b", RelationKind::ReportsTo),
            Err(OrgError::Duplicate)
        );
        assert_eq!(
            m.add_relation("a", "a", RelationKind::Delegates),
            Err(OrgError::SelfRelation)
        );
        let id = m.relations[0].id.clone();
        m.remove_relation(&id).unwrap();
        assert!(m.remove_relation(&id).is_err());
    }
}
