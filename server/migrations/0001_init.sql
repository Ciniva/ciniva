CREATE TABLE movies (
    id INTEGER PRIMARY KEY,
    tmdb_id INTEGER NOT NULL UNIQUE,
    title TEXT NOT NULL,
    original_title TEXT,
    year INTEGER,
    overview TEXT,
    tagline TEXT,
    poster_path TEXT,
    backdrop_path TEXT,
    runtime INTEGER,
    rating REAL,
    genres TEXT NOT NULL DEFAULT '[]',
    release_date TEXT,
    cast_json TEXT NOT NULL DEFAULT '[]',
    added_at TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE TABLE shows (
    id INTEGER PRIMARY KEY,
    tmdb_id INTEGER NOT NULL UNIQUE,
    title TEXT NOT NULL,
    original_title TEXT,
    year INTEGER,
    overview TEXT,
    poster_path TEXT,
    backdrop_path TEXT,
    rating REAL,
    genres TEXT NOT NULL DEFAULT '[]',
    first_air_date TEXT,
    status TEXT,
    cast_json TEXT NOT NULL DEFAULT '[]',
    added_at TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE TABLE seasons (
    id INTEGER PRIMARY KEY,
    show_id INTEGER NOT NULL REFERENCES shows(id) ON DELETE CASCADE,
    season_number INTEGER NOT NULL,
    name TEXT,
    overview TEXT,
    poster_path TEXT,
    air_date TEXT,
    episode_count INTEGER,
    UNIQUE (show_id, season_number)
);

CREATE TABLE episodes (
    id INTEGER PRIMARY KEY,
    show_id INTEGER NOT NULL REFERENCES shows(id) ON DELETE CASCADE,
    season_number INTEGER NOT NULL,
    episode_number INTEGER NOT NULL,
    title TEXT,
    overview TEXT,
    still_path TEXT,
    air_date TEXT,
    runtime INTEGER,
    rating REAL,
    UNIQUE (show_id, season_number, episode_number)
);

CREATE TABLE files (
    id INTEGER PRIMARY KEY,
    path TEXT NOT NULL UNIQUE,
    size INTEGER NOT NULL,
    mtime INTEGER NOT NULL,
    guess TEXT NOT NULL,
    kind TEXT NOT NULL DEFAULT 'pending',
    movie_id INTEGER REFERENCES movies(id) ON DELETE SET NULL,
    match_note TEXT,
    duration REAL,
    probe TEXT,
    added_at TEXT NOT NULL DEFAULT (datetime('now'))
);
CREATE INDEX files_kind ON files(kind);
CREATE INDEX files_movie ON files(movie_id);

CREATE TABLE file_episodes (
    file_id INTEGER NOT NULL REFERENCES files(id) ON DELETE CASCADE,
    episode_id INTEGER NOT NULL REFERENCES episodes(id) ON DELETE CASCADE,
    PRIMARY KEY (file_id, episode_id)
);
CREATE INDEX file_episodes_episode ON file_episodes(episode_id);

CREATE TABLE manual_matches (
    path TEXT PRIMARY KEY,
    kind TEXT NOT NULL,
    tmdb_id INTEGER NOT NULL,
    season INTEGER,
    episode INTEGER
);

CREATE TABLE progress (
    file_id INTEGER PRIMARY KEY REFERENCES files(id) ON DELETE CASCADE,
    position_ms INTEGER NOT NULL,
    duration_ms INTEGER NOT NULL,
    watched INTEGER NOT NULL DEFAULT 0,
    updated_at TEXT NOT NULL DEFAULT (datetime('now'))
);
