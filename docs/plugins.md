# Ciniva plugin protocol (v1)

A plugin is an HTTP service in its own container. Ciniva never runs plugin code; it talks to plugins over HTTP and
renders the screens they describe. Every client (web, macOS, Windows) draws the same declarative views natively.

## Environment a plugin receives

| Variable | Meaning |
|---|---|
| `CINIVA_URL` | How the plugin reaches Ciniva, e.g. `http://ciniva:8484` |
| `CINIVA_PLUGIN_SECRET` | Shared service key; send as `Authorization: Bearer …` to Ciniva |
| `PLUGIN_URL` | How Ciniva reaches the plugin, e.g. `http://ciniva-qbittorrent:8090` |

## Registration and heartbeat

`POST {CINIVA_URL}/api/plugins/register` with the service key, every 60 s (first call at startup):

```json
{
  "id": "qbittorrent",
  "name": "qBittorrent",
  "version": "0.1.0",
  "description": "Downloads, search and Download & watch",
  "icon": "download",
  "url": "http://ciniva-qbittorrent:8090",
  "settings": [Field],
  "pages": [{ "id": "downloads", "title": "Downloads", "icon": "download", "nav": true }],
  "actions": [{ "id": "find_missing", "title": "Find missing episodes", "icon": "search", "contexts": ["show", "season"] }]
}
```

Response: `{ "settings": { "<field name>": value, … } }` (the values an administrator saved; defaults from `Field.value`).
A plugin not heard from for 3 minutes is shown as offline.

## Ciniva → plugin

Clients call `/api/plugins/{id}/ui/{path}` on Ciniva (any method, query and JSON body). Ciniva forwards it to
`{plugin url}/ui/{path}` with these headers added:

- `X-Ciniva-Secret`: the service key (plugins must reject calls without it)
- `X-Ciniva-User`: username of the signed-in user
- `X-Ciniva-Admin`: `1` or `0`

Conventional paths:

- `GET ui/pages/{page}?…` → `View`
- `POST ui/actions/{action}` with `{ "context": Context }` → `Result`
- anything a `Call` names → `Result`

`Context` for actions: `{ "kind": "show"|"season"|"movie", "id": <ciniva id>, "tmdb_id", "title", "year", "season"?, "missing"?: [{ "season", "episode", "title" }] }`

## Views

```
View    = { title, subtitle?, refresh?: seconds, blocks: [Block] }
Block   = { type: "text",    text, tone? }
        | { type: "stats",   items: [{ label, value }] }
        | { type: "actions", items: [Action] }
        | { type: "form",    id?, fields: [Field], submit: { label, call: Call } }
        | { type: "list",    id?, items: [Item], empty?: string }
Item    = { id, title, subtitle?, image?, progress?: 0..1, badges?: [{ text, tone? }], actions?: [Action], link?: Nav }
Action  = { label, icon?, tone?: "primary"|"danger"|"default", confirm?: string, call?: Call, navigate?: Nav }
Nav     = { page, params?: { … } }
Call    = { method: "GET"|"POST"|"PUT"|"DELETE", path, body?: { … } }
Field   = { name, label, kind: "text"|"password"|"number"|"select"|"toggle"|"file"|"textarea",
            value?, placeholder?, help?, required?: bool, options?: [{ value, label }] }
Result  = { message?, tone?: "info"|"success"|"error", view?: View, navigate?: Nav, refresh?: bool }
tone    = "info" | "success" | "warning" | "error"
```

- `Call.path` is relative to the plugin's `ui/` root (`"torrents/abc/pause"` → `ui/torrents/abc/pause`).
- Form submit sends `call.body` merged with the field values. A `file` field is sent as
  `{ "name": "<file name>", "data_base64": "…" }`.
- `View.refresh` asks the client to re-fetch the page every N seconds while it is visible (live progress).
- After a `Result`: show `message` as a toast, then replace the view with `view`, or go to `navigate`, or re-fetch the
  current page when `refresh` is true.

## Plugin → Ciniva (service key)

- `POST /api/scan` with optional `{ "paths": ["/media/Downloads/Show.S01E01.mkv"] }`: rescan only those paths (files or folders).
- `POST /api/notifications` with `{ "title", "body"?, "tone"? }`: shown to signed-in clients.
- `POST /api/downloads/report` with `{ "plugin", "items": [{ "id", "title", "path", "size", "ranges": [[start, end], …], "progress" }] }`:
  files that can be watched while downloading. `path` is the partial file inside a media folder, `ranges` are the
  byte ranges already on disk. Report every few seconds; an item disappears 60 s after its last report.
- All library endpoints (`/api/shows/{id}`, `/api/shows/{id}/seasons/{n}`, `/api/search`, …).

## Clients

- `GET /api/plugins` → `[{ id, name, version, description, icon, online, pages, actions, has_settings }]`
- `GET /api/plugins/{id}/settings` → `{ fields: [Field with current value] }` (administrators)
- `PUT /api/plugins/{id}/settings` with `{ values: { … } }` (administrators)
- `GET /api/events` (Server-Sent Events): `notification` → `{ title, body?, tone?, plugin? }`, `library` → `{ kind: "scan.finished" }`
