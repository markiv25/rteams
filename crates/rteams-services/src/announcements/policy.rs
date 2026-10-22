//! Which announcements are shown, and which one (if any) opens automatically.

use super::feed::Announcement;
use super::state::State;

fn ver(v: &str) -> Option<semver::Version> {
    semver::Version::parse(v.trim().trim_start_matches(['v', 'V'])).ok()
}

/// Published, and within the item's `min_app_version..=max_app_version` window.
pub fn applies(a: &Announcement, now_ms: i64, app_version: &str) -> bool {
    if a.published_at > now_ms {
        return false;
    }
    let app = ver(app_version);
    if let (Some(min), Some(app)) = (a.min_app_version.as_deref().and_then(ver), app.as_ref()) {
        if app < &min {
            return false;
        }
    }
    if let (Some(max), Some(app)) = (a.max_app_version.as_deref().and_then(ver), app.as_ref()) {
        if app > &max {
            return false;
        }
    }
    true
}

pub fn is_expired(a: &Announcement, now_ms: i64) -> bool {
    a.expires_at.is_some_and(|e| e <= now_ms)
}

/// Items for the news list, newest first. Expired items stay listed (flagged "No longer
/// current" by the UI); unpublished and out-of-version-range items do not.
pub fn listed<'a>(
    items: &'a [Announcement],
    now_ms: i64,
    app_version: &str,
) -> Vec<&'a Announcement> {
    let mut v: Vec<&Announcement> = items
        .iter()
        .filter(|a| applies(a, now_ms, app_version))
        .collect();
    v.sort_by_key(|a| std::cmp::Reverse(a.published_at));
    v
}

fn counts_as_unread(a: &Announcement, st: &State, now_ms: i64, app_version: &str) -> bool {
    applies(a, now_ms, app_version)
        && !is_expired(a, now_ms)
        && !st.is_read(&a.id)
        && !st.is_dismissed(&a.id)
        && (a.show_to_new_users || a.published_at >= st.first_app_opened_at)
}

pub fn unread_count(items: &[Announcement], st: &State, now_ms: i64, app_version: &str) -> usize {
    items
        .iter()
        .filter(|a| counts_as_unread(a, st, now_ms, app_version))
        .count()
}

/// The newest unseen, current item to open on startup (`None` = stay quiet).
pub fn select_auto<'a>(
    items: &'a [Announcement],
    st: &State,
    now_ms: i64,
    app_version: &str,
) -> Option<&'a Announcement> {
    items
        .iter()
        .filter(|a| counts_as_unread(a, st, now_ms, app_version) && !st.is_auto_claimed(&a.id))
        .max_by_key(|a| a.published_at)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn a(id: &str, published: i64) -> Announcement {
        Announcement {
            id: id.into(),
            title: id.into(),
            summary: String::new(),
            body: String::new(),
            published_at: published,
            expires_at: None,
            min_app_version: None,
            max_app_version: None,
            show_to_new_users: true,
            cover_url: None,
            url: None,
        }
    }

    #[test]
    fn publish_and_version_window() {
        let mut x = a("x", 100);
        assert!(applies(&x, 100, "1.0.0"));
        assert!(!applies(&x, 99, "1.0.0"), "not yet published");
        x.min_app_version = Some("1.2.0".into());
        assert!(!applies(&x, 200, "1.1.9"));
        assert!(applies(&x, 200, "1.2.0"));
        x.max_app_version = Some("1.5.0".into());
        assert!(!applies(&x, 200, "1.6.0"));
    }

    #[test]
    fn expiry_listing_and_unread() {
        let mut old = a("old", 10);
        old.expires_at = Some(50);
        let items = vec![old, a("new", 90), a("future", 500)];
        let st = State {
            first_app_opened_at: 1,
            ..State::default()
        };
        let l: Vec<_> = listed(&items, 100, "1.0.0")
            .iter()
            .map(|x| x.id.as_str())
            .collect();
        assert_eq!(
            l,
            ["new", "old"],
            "expired still listed; future hidden; newest first"
        );
        assert_eq!(unread_count(&items, &st, 100, "1.0.0"), 1);
        assert_eq!(select_auto(&items, &st, 100, "1.0.0").unwrap().id, "new");
    }

    #[test]
    fn read_dismissed_and_new_user_gate() {
        let mut gated = a("g", 10);
        gated.show_to_new_users = false;
        let items = vec![gated, a("n", 20)];
        let mut st = State {
            first_app_opened_at: 15,
            ..State::default()
        };
        assert_eq!(select_auto(&items, &st, 100, "1.0.0").unwrap().id, "n");
        st.mark_read("n");
        assert!(
            select_auto(&items, &st, 100, "1.0.0").is_none(),
            "gated item hidden from new users"
        );
        assert_eq!(unread_count(&items, &st, 100, "1.0.0"), 0);
    }
}
