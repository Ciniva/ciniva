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
