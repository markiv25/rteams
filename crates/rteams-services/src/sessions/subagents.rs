//! Subagent transcripts: `<project>/<session>/subagents/agent-<id>.jsonl`, linked to the
//! `Task` / `Agent` tool call that spawned them (by the agent id in the tool result, or by
//! matching the call's prompt with the transcript's first message).

use std::path::{Path, PathBuf};

use super::chunks::{build_chunks, parse_file, Aggregate, Chunk, Totals};
use super::paths::{subagent_files, Source};
use super::steps::StepKind;

#[derive(Debug, Clone)]
pub struct Subagent {
    pub agent_id: String,
    pub file: PathBuf,
    /// `tool_use` id of the parent call, once linked.
    pub parent_tool_id: Option<String>,
    pub description: String,
    pub prompt: String,
    pub chunks: Vec<Chunk>,
    pub totals: Totals,
    pub tool_count: u64,
    pub start_ms: Option<i64>,
    pub end_ms: Option<i64>,
}

impl Subagent {
    pub fn duration_ms(&self) -> Option<i64> {
        match (self.start_ms, self.end_ms) {
            (Some(a), Some(b)) if b >= a => Some(b - a),
            _ => None,
        }
    }
}

fn agent_id_of(file: &Path) -> String {
    let stem = file
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    stem.strip_prefix("agent-").unwrap_or(&stem).to_string()
}

/// Parse every subagent transcript of the session at `session_file`.
pub fn load_all(session_file: &Path, source: Source) -> Vec<Subagent> {
    subagent_files(session_file)
        .into_iter()
        .filter_map(|f| load_one(&f, source))
        .collect()
}

pub fn load_one(file: &Path, source: Source) -> Option<Subagent> {
    let mut msgs = Vec::new();
    parse_file(file, source, |m| msgs.push(m)).ok()?;
    if msgs.is_empty() {
        return None;
    }
    let mut agg = Aggregate::default();
    // Aggregate skips sidechain records; subagent transcripts are all sidechain.
    for m in &msgs {
        let mut m = m.clone();
        m.sidechain = false;
        agg.push(&m);
    }
    let chunks = build_chunks(&msgs, true);
    let prompt = chunks
        .first()
        .and_then(|c| c.prompt.clone())
        .unwrap_or_default();
    Some(Subagent {
        agent_id: agent_id_of(file),
        file: file.to_path_buf(),
        parent_tool_id: None,
        description: String::new(),
        prompt,
        chunks,
        totals: agg.totals(),
        tool_count: agg.tool_count,
        start_ms: agg.start_ms,
        end_ms: agg.end_ms,
    })
}

/// Turn `Task` / `Agent` tool steps that own a transcript into `Subagent` steps.
pub fn link(chunks: &mut [Chunk], subs: &mut [Subagent]) {
    let mut used = vec![false; subs.len()];
    for c in chunks.iter_mut() {
        for s in c.steps.iter_mut() {
            let Some(t) = s.tool.as_ref() else { continue };
            if !t.is_task() {
                continue;
            }
            let by_id = t
                .agent_id
                .as_deref()
                .and_then(|a| subs.iter().position(|x| x.agent_id == a));
            let by_prompt = || {
                let p = t.input.get("prompt").and_then(|v| v.as_str())?;
                subs.iter().enumerate().position(|(i, x)| {
                    !used[i] && !x.prompt.is_empty() && x.prompt.trim() == p.trim()
                })
            };
            if let Some(i) = by_id.or_else(by_prompt) {
                used[i] = true;
                subs[i].parent_tool_id = Some(t.id.clone());
                subs[i].description = t
                    .input
                    .get("description")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                s.kind = StepKind::Subagent;
                s.subagent = Some(i);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::chunks::load_session;
    use super::*;

    #[test]
    fn links_by_agent_id_and_prompt() {
        let d = tempfile::tempdir().unwrap();
        let proj = d.path().join("-w");
        std::fs::create_dir_all(proj.join("s1/subagents")).unwrap();
        let main = [
            r#"{"type":"user","uuid":"u1","sessionId":"s1","timestamp":"2025-03-01T10:00:00Z","message":{"role":"user","content":"do it"}}"#,
            r#"{"type":"assistant","uuid":"a1","timestamp":"2025-03-01T10:00:01Z","message":{"id":"m1","model":"claude-sonnet-4-5","content":[{"type":"tool_use","id":"tk1","name":"Task","input":{"description":"explore","prompt":"look around"}},{"type":"tool_use","id":"tk2","name":"Task","input":{"description":"other","prompt":"second job"}}],"usage":{"input_tokens":1,"output_tokens":1}}}"#,
            r#"{"type":"user","uuid":"u2","timestamp":"2025-03-01T10:00:30Z","toolUseResult":{"agentId":"abc"},"message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"tk1","content":"done"}]}}"#,
            r#"{"type":"user","uuid":"u3","timestamp":"2025-03-01T10:00:31Z","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"tk2","content":"done"}]}}"#,
        ]
        .join("\n");
        std::fs::write(proj.join("s1.jsonl"), main).unwrap();
        let sub = |p: &str| {
            [
                format!(r#"{{"type":"user","isSidechain":true,"timestamp":"2025-03-01T10:00:02Z","message":{{"role":"user","content":"{p}"}}}}"#),
                r#"{"type":"assistant","isSidechain":true,"timestamp":"2025-03-01T10:00:20Z","message":{"id":"sm","model":"claude-haiku-4-5","content":[{"type":"tool_use","id":"x","name":"Grep","input":{"pattern":"foo"}}],"usage":{"input_tokens":50,"output_tokens":10}}}"#.to_string(),
            ]
            .join("\n")
        };
        std::fs::write(
            proj.join("s1/subagents/agent-abc.jsonl"),
            sub("look around"),
        )
        .unwrap();
        std::fs::write(proj.join("s1/subagents/agent-zzz.jsonl"), sub("second job")).unwrap();
        let s = load_session(&proj.join("s1.jsonl"), Source::Claude).unwrap();
        assert_eq!(s.subagents.len(), 2);
        let steps = &s.chunks[0].steps;
        assert!(steps.iter().all(|x| x.kind == StepKind::Subagent));
        let a = &s.subagents[steps[0].subagent.unwrap()];
        assert_eq!(a.agent_id, "abc");
        assert_eq!(a.description, "explore");
        assert_eq!(a.tool_count, 1);
        assert_eq!(a.duration_ms(), Some(18_000));
        assert_eq!(s.subagents[steps[1].subagent.unwrap()].agent_id, "zzz");
        assert!(s.grand_totals().usage.input >= 101);
    }
}
