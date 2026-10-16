//! Usage summaries (read-only), computed from the same ledger scan the GUI uses.

use serde_json::{json, Value};

use super::{Ctx, Req, Resp};
use crate::usage::ledger::{now_ms, Scanner};
use crate::usage::summary::{summarize, Filter, Row, Summary};

fn row(r: &Row) -> Value {
    json!({ "key": r.key, "input": r.input, "output": r.output, "cache": r.cache,
            "reasoning": r.reasoning, "tokens": r.tokens, "cost": r.cost,
            "requests": r.requests, "runs": r.runs })
}

fn summary_json(s: &Summary) -> Value {
    let t = &s.totals;
    json!({
        "totals": { "input": t.input, "output": t.output, "cache_creation": t.cache_creation,
                    "cache_read": t.cache_read, "reasoning": t.reasoning, "tokens": t.tokens,
                    "cost": t.cost, "requests": t.requests, "runs": t.runs },
        "by_day": s.by_day.iter().map(|d| json!({"day_ms": d.day_ms, "tokens": d.tokens, "cost": d.cost, "requests": d.requests})).collect::<Vec<_>>(),
        "by_team": s.by_team.iter().map(row).collect::<Vec<_>>(),
        "by_member": s.by_member.iter().map(row).collect::<Vec<_>>(),
        "by_model": s.by_model.iter().map(row).collect::<Vec<_>>(),
        "burn": { "daily_cost": s.burn.daily_cost, "forecast_week": s.burn.forecast_week,
                  "forecast_month": s.burn.forecast_month },
    })
}

pub fn handle(ctx: &Ctx, r: &Req) -> Option<Resp> {
    let team = match (r.method.as_str(), r.segs().as_slice()) {
        ("GET", ["api", "usage"]) => None,
        ("GET", ["api", "teams", t, "usage"]) => {
            if !super::routes_teams::valid_team(t) {
                return Some(Resp::err(400, "invalid team name"));
            }
            Some((*t).to_string())
        }
        _ => return None,
    };
    let ledger = Scanner::app_only(&ctx.home).scan();
    let mut f = Filter {
        include_cache: r.q("cache") == Some("1"),
        ..Filter::default()
    };
    if let Some(t) = team {
        f.teams.insert(t);
    }
    Some(Resp::ok(summary_json(&summarize(
        &ledger.records,
        &f,
        now_ms(),
    ))))
}
