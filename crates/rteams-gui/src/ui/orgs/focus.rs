//! Search and focus: find a node, highlight its path to the root, descendants and connected
//! nodes, and dim everything else.

use eframe::egui::{self, Align2, Color32, RichText};
use rteams_services::i18n::I18n;

use super::layout::{Key, Layout, RelEdge};
use crate::theme;

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Focus {
    pub query: String,
    pub target: Option<Key>,
}

impl Focus {
    pub fn clear(&mut self) {
        self.query.clear();
        self.target = None;
    }

    pub fn is_active(&self) -> bool {
        self.target.is_some() || !self.query.trim().is_empty()
    }

    /// Node indices to highlight seeds from: the focus target, else the search matches.
    pub fn seeds(&self, l: &Layout) -> Vec<usize> {
        if let Some(i) = self.target.as_ref().and_then(|k| l.find(k)) {
            return vec![i];
        }
        search(l, &self.query)
    }

    /// Per-node "stay bright" flags, or `None` when nothing is focused.
    pub fn highlight(&self, l: &Layout, rels: &[RelEdge]) -> Option<Vec<bool>> {
        if !self.is_active() {
            return None;
        }
        Some(highlight(l, &self.seeds(l), rels))
    }
}

/// Case-insensitive substring search over labels and ids; prefix matches first.
pub fn search(l: &Layout, query: &str) -> Vec<usize> {
    let q = query.trim().to_lowercase();
    if q.is_empty() {
        return Vec::new();
    }
    let mut hits: Vec<(bool, usize)> = l
        .nodes
        .iter()
        .enumerate()
        .filter(|(_, n)| !matches!(n.key, Key::Root))
        .filter_map(|(i, n)| {
            let label = n.label.to_lowercase();
            let id = match &n.key {
                Key::Org(i) | Key::Team(i) => i.to_lowercase(),
                Key::Group(_, g) => g.to_lowercase(),
                _ => String::new(),
            };
            (label.contains(&q) || id.contains(&q))
                .then(|| (!(label.starts_with(&q) || id.starts_with(&q)), i))
        })
        .collect();
    hits.sort();
    hits.into_iter().map(|(_, i)| i).collect()
}

pub fn path_to_root(l: &Layout, mut i: usize) -> Vec<usize> {
    let mut v = vec![i];
    while let Some(p) = l.nodes[i].parent {
        v.push(p);
        i = p;
    }
    v
}

pub fn highlight(l: &Layout, seeds: &[usize], rels: &[RelEdge]) -> Vec<bool> {
    let mut on = vec![false; l.nodes.len()];
    for &s in seeds {
        for p in path_to_root(l, s) {
            on[p] = true;
        }
        for (i, o) in on.iter_mut().enumerate() {
            if l.is_ancestor(s, i) {
                *o = true;
            }
        }
        for r in rels {
            let other = if r.a == s {
                r.b
            } else if r.b == s {
                r.a
            } else {
                continue;
            };
            for p in path_to_root(l, other) {
                on[p] = true;
            }
        }
    }
    on
}

pub fn kind_label(tr: &I18n, k: &Key) -> String {
    match k {
        Key::Org(_) | Key::Root => tr.t("team.organizations.graph.focus.kind.organization"),
        Key::Group(..) | Key::Unplaced => tr.t("team.organizations.graph.focus.kind.container"),
        Key::Team(_) => tr.t("team.organizations.graph.focus.kind.team"),
    }
}

/// Dropdown of search results under the search box; returns the picked node.
pub fn show_results(
    ctx: &egui::Context,
    tr: &I18n,
    l: &Layout,
    matches: &[usize],
    anchor: egui::Pos2,
) -> Option<usize> {
    let mut picked = None;
    egui::Area::new(egui::Id::unique("orgmap_search_results"))
        .order(egui::Order::Foreground)
        .pivot(Align2::LEFT_TOP)
        .fixed_pos(anchor)
        .show(ctx, |ui| {
            egui::Frame::popup(ui.style()).show(ui, |ui| {
                ui.set_max_width(280.0);
                if matches.is_empty() {
                    ui.label(
                        RichText::new(tr.t("team.organizations.graph.focus.noResults"))
                            .color(theme::MUTED),
                    );
                }
                for &i in matches.iter().take(8) {
                    let n = &l.nodes[i];
                    let text = format!("{}  ({})", n.label, kind_label(tr, &n.key));
                    if ui
                        .selectable_label(false, RichText::new(text).color(Color32::WHITE))
                        .clicked()
                    {
                        picked = Some(i);
                    }
                }
            });
        });
    picked
}

#[cfg(test)]
mod tests {
    use super::super::layout::{nested, tests::view};
    use super::*;

    #[test]
    fn orgs_focus_search_and_highlight() {
        let v = view();
        let l = nested(&v);
        let hits = search(&l, "squad");
        assert_eq!(hits.len(), 2);
        assert!(search(&l, "  ").is_empty());
        assert!(search(&l, "zzzz").is_empty());
        // Prefix matches sort first.
        let t = search(&l, "team1");
        assert!(matches!(&l.nodes[t[0]].key, Key::Team(d) if d.starts_with("team1")));
        // Highlight keeps the path to root, descendants and related nodes only.
        let t0 = l.find(&Key::Team("team0".into())).unwrap();
        let t1 = l.find(&Key::Team("team1".into())).unwrap();
        let t5 = l.find(&Key::Team("team5".into())).unwrap();
        let rels = l.relation_edges(&v);
        let on = highlight(&l, &[t0], &rels);
        assert!(on[t0] && on[t1], "relation partner stays lit");
        assert!(!on[t5]);
        let parent = l.nodes[t0].parent.unwrap();
        assert!(on[parent]);
        let org = *path_to_root(&l, t0).last().unwrap();
        let all = highlight(&l, &[org], &rels);
        assert!(all[t0]);
        let mut f = Focus::default();
        assert!(f.highlight(&l, &rels).is_none());
        f.query = "gamma".into();
        assert!(f.highlight(&l, &rels).is_some());
        f.clear();
        assert!(!f.is_active());
    }
}
