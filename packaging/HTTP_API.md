# rteams local HTTP API

A small JSON + Server-Sent-Events API for dashboards and scripts. It is **off by default**.

* Enable: Settings, or `config.json` -> `general.server_enabled = true` (`server_port`, `server_token`).
  Port `0` picks a free port. An empty `server_token` is replaced by a random 256-bit token.
* Binds `127.0.0.1` only. The `Host` header must be `127.0.0.1`, `localhost` or `[::1]`.
* **Every** route (including 404s) needs `Authorization: Bearer <server_token>`; compared in constant time.
  Missing or wrong token: `401`. No CORS headers are sent, so browsers cannot call it cross-origin.
* Request bodies are capped at 1 MiB (`413`). At most 64 concurrent connections (`503`).
* Reads use the read-only event store. Writes (POST) go through `rteams_server::Server::call_tool`, the
  same code path and rules as agent tool calls, and **require a `command_id`** so retries are idempotent.
  Only the POST routes listed below exist; member changes, deletes and permission decisions are not exposed.

```
curl -H "Authorization: Bearer $TOKEN" http://127.0.0.1:7860/api/teams
```

## Routes

| Method | Path | Result |
|---|---|---|
| GET | `/api/health` | `{"ok":true}` |
| GET | `/api/version` | `{"name","version"}` |
| GET | `/api/teams` | `{"teams":[{"name","members","tasks","last_seq"}]}` |
| GET | `/api/teams/:t` | team record plus counts and `last_seq` |
| GET | `/api/teams/:t/tasks[?archived=1]` | `{"tasks":[...]}` |
| GET | `/api/teams/:t/kanban` | `{"board":{todo,in_progress,review,approved,done}}` |
| GET | `/api/teams/:t/messages[?limit=100&after=<seq>]` | `{"messages":[...]}` |
| GET | `/api/teams/:t/events[?limit=200&after=<seq>]` | raw event log slice |
| GET | `/api/teams/:t/usage` | usage summary for one team |
| GET | `/api/usage[?cache=1]` | usage summary for all teams (totals, by_day, by_team, by_member, by_model, burn) |
| GET | `/api/config` | app config with every token/secret/key redacted as `"***"` |
| GET | `/api/events[?team=<t>]` | SSE stream, see below |
| POST | `/api/teams/:t/tasks` | `task_create` args: `{"actor","subject","description?","owner?","blocked_by?","command_id"}` |
| POST | `/api/teams/:t/messages` | `message_send` args: `{"actor","to?","text","mode?","command_id"}` |
| POST | `/api/teams/:t/comments` | `task_add_comment` args (with `command_id`) |

Errors are `{"error":"..."}` with `400` (bad input / missing `command_id`), `401`, `403` (bad Host),
`404`, `405`, `413`, `422` (the tool refused, e.g. non-member actor), `503`.

## SSE: `GET /api/events`

Starts at the current end of each team's log (no replay); use `/events?after=` to catch up. Each new event:

```
id: <team>:<seq>
event: event
data: {"team":"<t>","seq":<n>,"event":{...}}
```

A `: keepalive` comment is sent every 15 s. Browsers' `EventSource` cannot set headers, so use `fetch`
streaming or `curl -N`. Authorization is never accepted via the query string.
