# rteams

A small team-coordination server for AI agents (tasks, review, messages, briefings), spoken over
MCP on stdio. Each agent's Claude Code starts its own `rteams serve` process; all of them share
one durable event log, so the team shares one board.

## Use it with Claude Code

Tested with Claude Code 2.1.301: the server reports `connected` and every tool is exposed to the
model as `mcp__rteams__<tool>`.

### 1. Install the binary

```sh
cargo install --path crates/rteams-cli   # puts `rteams` in ~/.cargo/bin
```

(Or use the absolute path to `target/debug/rteams` after `cargo build`.)

### 2a. Project-local `.mcp.json` (checked in, shared by the whole team)

```json
{
  "mcpServers": {
    "rteams": {
      "command": "rteams",
      "args": ["serve", "--home", "/abs/path/to/shared/rteams-home", "--team", "my-team"]
    }
  }
}
```

The same thing through environment variables:

```json
{
  "mcpServers": {
    "rteams": {
      "command": "rteams",
      "args": ["serve"],
      "env": { "RTEAMS_HOME": "/abs/path/to/shared/rteams-home", "RTEAMS_TEAM": "my-team" }
    }
  }
}
```

Claude Code asks you to approve a project-scoped server the first time you run `claude` in that
directory (`claude mcp list` shows it as "Pending approval" until then).

### 2b. Or register it with the CLI

```sh
claude mcp add rteams -e RTEAMS_HOME=/abs/path/to/shared/rteams-home -e RTEAMS_TEAM=my-team \
  -- rteams serve
# add --scope project to write .mcp.json, --scope user to make it available everywhere
claude mcp list        # runs a health check; expect "rteams: ... - Connected"
```

### 3. Allow the tools (optional)

```sh
claude --allowedTools "mcp__rteams"      # all rteams tools, no per-call prompts
```

## Configuration

`rteams [serve] [--home DIR] [--team NAME]`

| Flag / env | Meaning |
|---|---|
| `--home DIR` / `RTEAMS_HOME` | Shared state root. State lives in `<home>/<team>/events.jsonl`. Omit for in-memory state private to that process (fine for a demo, useless for several agents). |
| `--team NAME` / `RTEAMS_TEAM` | Team directory under the home. Default `default`. |

Flags win over environment variables. Every agent that should see the same board must use the
same home and team. Writes take a file lock, catch up with other processes, then validate against
the latest state, so concurrent agents are safe.

## Things to know when calling the tools

- **`actor` is explicit.** The server has no login: write tools take `actor` (your member name)
  on every call, and read tools take `member`. Put each agent's name in its prompt or
  `CLAUDE.md`.
- **First call is `team_create`.** The actor that creates the team joins as role `lead`; list the
  other members in the same call. Do it once per team directory.
- **Start of a session:** `member_briefing { member }` (team, roster, who leads, queue sizes,
  protocol), then `task_briefing { member }` for the work queue. `task_briefing` with
  `role: "lead"` adds the team-wide view and is only allowed for members with the lead role.
- **Idempotent writes.** Every write tool accepts an optional `command_id`; retrying with the same
  id returns the original result instead of appending a second event.
- **Errors.** Domain failures (`task_not_found`, `illegal_transition`, `forbidden`,
  `unknown_member`, ...) come back as tool results with `isError: true` and a `{code, message}`
  body, so the model can read and react. Malformed arguments and unknown tools are JSON-RPC
  `-32602` errors.
- **Tools:** `team_create`, `team_get`, `task_create`, `task_get`, `task_list`, `task_start`,
  `task_complete`, `task_set_dependencies`, `task_set_owner`, `task_add_comment`, `task_block`,
  `task_unblock`, `task_archive`, `task_briefing`, `member_briefing`, `kanban_get`,
  `review_request`, `review_approve`, `review_request_changes`, `message_send`, `inbox_get`,
  `process_register`, `process_list`, `process_unregister`. `tools/list` is the source of truth.

## Troubleshooting

- **Handshake fails / server shows "Failed to connect":** stdout must carry only JSON-RPC.
  rteams logs to stderr only (one line, `rteams: state in <dir>`). If you wrap the binary in a
  script, do not `echo` or let a shell profile print anything to stdout.
- **Tools never appear:** run `claude mcp list` in the project directory; "Pending approval"
  means you have not approved the `.mcp.json` server yet.
- **Agents do not see each other's tasks:** they are using different `--home` / `--team` values,
  or no home at all (in-memory state).
- Run the server by hand to inspect it:
  ```sh
  printf '%s\n' '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"t","version":"1"}}}' \
    '{"jsonrpc":"2.0","id":2,"method":"tools/list"}' | rteams serve
  ```

## Protocol

Hand-rolled JSON-RPC 2.0 over newline-delimited stdio. Supports `initialize` (negotiates
`2025-06-18`, `2025-03-26`, `2024-11-05`; an unknown newer client version gets `2025-06-18`),
`notifications/initialized`, `ping`, `tools/list`, `tools/call`. Advertised capabilities:
`tools` only; `resources/*` and `prompts/*` answer `-32601`. Successful tool results carry both
a text `content` block and `structuredContent`.

`crates/rteams-cli/tests/claude_code.rs` replays the Claude Code client sequence against the real
binary on every CI run.

## Development

`scripts/ci.sh` runs the same checks as CI: `cargo fmt --check`, `cargo clippy -D warnings`,
`cargo test --workspace`.
