# Ciniva

Self-hosted media server. Dump movies and shows into one folder; Ciniva works out what each file is, matches it on TMDB and builds the library.

## How files are classified

1. **File name** – `S01E02`, `1x02`, `Season 1 Episode 2`, dates (`2024.03.15`), anime `Show - 05` → episode. Title + year → movie.
2. **Folder context** – season-pack folders (`Show.S05.1080p…/08. Title.mkv`), `Show/Season 2/…`, and siblings that differ only by a number.
3. **TMDB** – confirms the match, settles sequels vs. episodes (`Shrek 2`), and maps absolute anime numbering to seasons.
4. **Runtime** – a 40-minute file is never accepted as a 2-hour movie.

Anything still uncertain lands in *Unmatched*, where a manual match is remembered by path.

## Run

```sh
cd server
CINIVA_MEDIA=/path/to/dump CINIVA_DATA=./data CINIVA_WEB=../web cargo run --release
```

Open the web UI. The first visit creates the administrator account, then asks for a TMDB API key (or read access
token); both can be changed later in Settings, where administrators also add users. Apps sign in with the same
username and password.

| Variable | Default | |
|---|---|---|
| `CINIVA_MEDIA` | `/media` | Comma-separated folders to scan |
| `CINIVA_DATA` | `./data` | Database and image cache |
| `TMDB_API_KEY` | – | Optional: TMDB key, if you'd rather not enter it during setup (a key saved in setup wins) |
| `CINIVA_PORT` | `8484` | |
| `CINIVA_PLUGIN_SECRET` | – | Service key plugins use (`Authorization: Bearer`); needed for plugins |
| `CINIVA_TOKEN` | – | Legacy service key with the same rights; prefer `CINIVA_PLUGIN_SECRET` |
| `CINIVA_LANGUAGE` | `en-US` | TMDB metadata language |
| `CINIVA_SCAN_INTERVAL` | `15` | Minutes between rescans |
| `CINIVA_WEB` | `/web` in Docker | Folder with the web UI (`web/`); unset serves the API only |

## Web UI

`web/` is a dependency-free single-page app (plain ES modules plus a vendored hls.js), served by the server at `/`. It covers browsing, search, playback and fixing unmatched files.

- Plays files directly when the browser supports the container and codecs; otherwise it asks the server for a GPU transcode over HLS (HEVC when the browser decodes it, H.264 otherwise; HDR kept on HDR displays, tone-mapped elsewhere).
- H.264 in containers browsers can't open (MKV, AVI, TS) is remuxed at Original quality: the video is copied untouched into HLS, cut on its own keyframes. Keyframes are indexed once per file (in the background thumbnail pass, or on first play, which transcodes that one time).
- Audio track, text subtitles (extracted to WebVTT on first use), quality, resume, seek thumbnails, up next.
- Keys: space play/pause, ←/→ 10 s, ⇧ 30 s, ⌘/Ctrl+⇧ 1 min, ↑/↓ volume, F full screen, M mute, Esc back.

## Docker

```sh
echo "MEDIA_ROOT=/path/to/share" >> .env          # holds Movies, TV and Downloads
echo "RENDER_GID=$(stat -c %g /dev/dri/renderD128)" >> .env
echo "CINIVA_PLUGIN_SECRET=$(openssl rand -hex 32)" >> .env
docker compose up -d                              # Ciniva only
docker compose --profile downloads up -d          # plus qBittorrent, Prowlarr and the qBittorrent plugin
```

The container never runs as root: it runs as an unprivileged user (UID/GID 1000 by default,
`CINIVA_UID`/`CINIVA_GID` to change), with every Linux capability dropped, `no-new-privileges`,
a read-only root filesystem and no setuid binaries in the image. Media is mounted read-only;
only `/data` is writable.

For a further layer, enable Docker user-namespace remapping (`"userns-remap": "default"` in
`/etc/docker/daemon.json`) so even the container's UID maps to an unused range on the host.
This applies to every container on the host, so it is not enabled by the compose file.

GPU access: Intel/AMD need `/dev/dri` and the host's render group (`RENDER_GID`); NVIDIA needs
the NVIDIA Container Toolkit. `docker exec ciniva ciniva-hwinfo` lists what was detected.

## Plugins

Plugins are separate containers that register with Ciniva over HTTP; Ciniva never runs their code. They describe
their settings, pages and actions declaratively and every client (web, macOS) draws them natively. Protocol:
[docs/plugins.md](docs/plugins.md). Plugins sit on an internal Docker network with no internet access and receive only
the service key, so a compromised plugin can reach Ciniva's API and nothing else.

- `plugins/qbittorrent`: a Downloads page with live progress, add magnet/URL/.torrent, pause/resume, sequential
  ("download in order"), per-file priorities, Prowlarr search ranked by your preferred quality, languages and size,
  and "Find episodes"/"Find releases" on show, season and movie pages. When a download finishes it rescans just that
  folder (it appears within seconds) and notifies signed-in clients.
- `plugins/jellyfin`: a Jellyfin-compatible API on port 8096 so Jellyfin apps (Android TV, webOS, Tizen, Roku), Swiftfin,
  Infuse and Findroid can browse, play and sync progress. Add the server as `http://<server>:8096` and sign in with your
  Ciniva account.

## Downloads stack and its security

qBittorrent and Prowlarr run in their own containers built from pinned, checksum-verified releases
(`docker/qbittorrent`, `docker/prowlarr`), unprivileged and with read-only root filesystems:

- qBittorrent sees the library read-only, so it can keep seeding existing torrents but cannot alter or delete them.
  It can write only to `Downloads`, which Ciniva also scans. Incomplete files carry the `.!qB` extension, so Ciniva
  never imports a half-downloaded file.
- qBittorrent lets the internal network use its API without a password (for the plugin); its Web UI on port 8080 still
  needs your login. Prowlarr's Web UI (port 9696) asks for a login on first visit; add your trackers there.

## Apps

- macOS: `apps/macos` (`./build.sh`, then `open build/Ciniva.app`). Direct-plays every format through mpv or
  AVFoundation (Dolby Vision, HDR10, Atmos), with resume, credits/Up next, episode navigation and plugin pages.
