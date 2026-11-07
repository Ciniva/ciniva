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
TMDB_API_KEY=… CINIVA_MEDIA=/path/to/dump CINIVA_DATA=./data cargo run --release
```

| Variable | Default | |
|---|---|---|
| `CINIVA_MEDIA` | `/media` | Comma-separated folders to scan |
| `CINIVA_DATA` | `./data` | Database and image cache |
| `TMDB_API_KEY` | – | TMDB v3 key or v4 read token |
| `CINIVA_PORT` | `8484` | |
| `CINIVA_TOKEN` | – | If set, required as `Authorization: Bearer` or `?token=` |
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
cp docker-compose.yml docker-compose.override.yml   # set your media path
echo "RENDER_GID=$(stat -c %g /dev/dri/renderD128)" >> .env
docker compose up -d
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
