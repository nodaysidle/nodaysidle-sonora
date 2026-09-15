use rusqlite::{params, Connection, OptionalExtension, Result, Row};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct TrackRecord {
    pub id: String,
    pub provider: String,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub album_artist: Option<String>,
    pub duration_ms: u64,
    pub year: Option<u32>,
    pub genre: Option<String>,
    pub file_path: Option<String>,
    pub artwork_url: Option<String>,
    pub bitrate: Option<u32>,
    pub format: Option<String>,
    pub loudness_lufs: Option<f64>,
    pub track_number: Option<u32>,
    pub disc_number: Option<u32>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct AlbumRecord {
    pub provider: String,
    pub album: String,
    pub album_artist: Option<String>,
    pub year: Option<u32>,
    pub artwork_url: Option<String>,
    pub track_count: u32,
    pub duration_ms: u64,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ArtistRecord {
    pub provider: String,
    pub artist: String,
    pub album_count: u32,
    pub track_count: u32,
}

pub struct DbState {
    // ponytail: one connection behind a mutex; a local library's queries are sub-millisecond and
    // the scanner commits in batches so the lock is never held for long. Move to an r2d2 pool if
    // concurrent readers ever contend on a 100k+ track library.
    pub conn: Mutex<Connection>,
}

const SCHEMA: &str = r#"
PRAGMA journal_mode = WAL;
PRAGMA foreign_keys = ON;
PRAGMA synchronous = NORMAL;

CREATE TABLE IF NOT EXISTS tracks (
    id TEXT PRIMARY KEY,
    provider TEXT NOT NULL,
    title TEXT NOT NULL,
    artist TEXT NOT NULL,
    album TEXT NOT NULL,
    album_artist TEXT,
    duration_ms INTEGER NOT NULL,
    year INTEGER,
    genre TEXT,
    file_path TEXT,
    artwork_url TEXT,
    bitrate INTEGER,
    format TEXT,
    loudness_lufs REAL,
    track_number INTEGER,
    disc_number INTEGER,
    date_added INTEGER NOT NULL
);

CREATE VIRTUAL TABLE IF NOT EXISTS tracks_fts USING fts5(
    title,
    artist,
    album,
    content='tracks',
    content_rowid='rowid'
);

CREATE TRIGGER IF NOT EXISTS tracks_ai AFTER INSERT ON tracks BEGIN
    INSERT INTO tracks_fts(rowid, title, artist, album)
    VALUES (new.rowid, new.title, new.artist, new.album);
END;

CREATE TRIGGER IF NOT EXISTS tracks_ad AFTER DELETE ON tracks BEGIN
    INSERT INTO tracks_fts(tracks_fts, rowid, title, artist, album)
    VALUES ('delete', old.rowid, old.title, old.artist, old.album);
END;

CREATE TRIGGER IF NOT EXISTS tracks_au AFTER UPDATE ON tracks BEGIN
    INSERT INTO tracks_fts(tracks_fts, rowid, title, artist, album)
    VALUES ('delete', old.rowid, old.title, old.artist, old.album);
    INSERT INTO tracks_fts(rowid, title, artist, album)
    VALUES (new.rowid, new.title, new.artist, new.album);
END;

CREATE INDEX IF NOT EXISTS idx_tracks_artist ON tracks(artist);
CREATE INDEX IF NOT EXISTS idx_tracks_album ON tracks(album);
CREATE INDEX IF NOT EXISTS idx_tracks_provider ON tracks(provider);

CREATE TABLE IF NOT EXISTS playlists (
    id TEXT PRIMARY KEY,
    title TEXT NOT NULL,
    description TEXT,
    artwork_url TEXT,
    created_at INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS playlist_tracks (
    playlist_id TEXT NOT NULL,
    track_id TEXT NOT NULL,
    position INTEGER NOT NULL,
    PRIMARY KEY (playlist_id, position),
    FOREIGN KEY(playlist_id) REFERENCES playlists(id) ON DELETE CASCADE,
    FOREIGN KEY(track_id) REFERENCES tracks(id) ON DELETE CASCADE
);

CREATE TABLE IF NOT EXISTS lyrics_cache (
    track_id TEXT PRIMARY KEY,
    raw_lrc TEXT NOT NULL,
    romanized_json TEXT,
    provider TEXT NOT NULL DEFAULT 'lrclib',
    cached_at INTEGER NOT NULL
);
"#;

const TRACK_COLUMNS: &str = "id, provider, title, artist, album, album_artist, duration_ms, year, \
     genre, file_path, artwork_url, bitrate, format, loudness_lufs, track_number, disc_number";

fn track_from_row(row: &Row) -> Result<TrackRecord> {
    Ok(TrackRecord {
        id: row.get(0)?,
        provider: row.get(1)?,
        title: row.get(2)?,
        artist: row.get(3)?,
        album: row.get(4)?,
        album_artist: row.get(5)?,
        duration_ms: row.get::<_, i64>(6)? as u64,
        year: row.get::<_, Option<i64>>(7)?.map(|v| v as u32),
        genre: row.get(8)?,
        file_path: row.get(9)?,
        artwork_url: row.get(10)?,
        bitrate: row.get::<_, Option<i64>>(11)?.map(|v| v as u32),
        format: row.get(12)?,
        loudness_lufs: row.get(13)?,
        track_number: row.get::<_, Option<i64>>(14)?.map(|v| v as u32),
        disc_number: row.get::<_, Option<i64>>(15)?.map(|v| v as u32),
    })
}

/// Escapes a user string into an FTS5 MATCH expression: every whitespace-separated token becomes a
/// quoted prefix term. Without this, characters like `-`, `*` or `"` are FTS5 syntax and error out.
fn fts_match_expression(query: &str) -> String {
    query
        .split_whitespace()
        .map(|token| format!("\"{}\"*", token.replace('"', "\"\"")))
        .collect::<Vec<_>>()
        .join(" ")
}

fn ensure_lyrics_provider_column(conn: &Connection) -> Result<()> {
    let mut stmt = conn.prepare("PRAGMA table_info(lyrics_cache)")?;
    let columns = stmt
        .query_map([], |row| row.get::<_, String>(1))?
        .collect::<Result<Vec<_>>>()?;
    drop(stmt);
    if !columns.iter().any(|column| column == "provider") {
        conn.execute(
            "ALTER TABLE lyrics_cache ADD COLUMN provider TEXT NOT NULL DEFAULT 'lrclib'",
            [],
        )?;
    }
    Ok(())
}

impl DbState {
    pub fn new(db_path: PathBuf) -> Result<Self> {
        if let Some(parent) = db_path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let conn = Connection::open(db_path)?;
        conn.execute_batch(SCHEMA)?;
        ensure_lyrics_provider_column(&conn)?;

        // A database created before the FTS triggers existed has rows in `tracks` that never made
        // it into the index. Rebuild once in that case.
        let tracks: i64 = conn.query_row("SELECT COUNT(*) FROM tracks", [], |r| r.get(0))?;
        let indexed: i64 = conn.query_row("SELECT COUNT(*) FROM tracks_fts", [], |r| r.get(0))?;
        if tracks > 0 && indexed == 0 {
            conn.execute("INSERT INTO tracks_fts(tracks_fts) VALUES('rebuild')", [])?;
        }

        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    /// Inserts or refreshes a batch of tracks. Returns how many rows were written.
    pub fn upsert_tracks(&self, tracks: &[TrackRecord]) -> Result<usize> {
        let mut conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let tx = conn.transaction()?;
        let now = chrono::Utc::now().timestamp();
        {
            let mut stmt = tx.prepare(
                "INSERT INTO tracks (id, provider, title, artist, album, album_artist, duration_ms,
                     year, genre, file_path, artwork_url, bitrate, format, loudness_lufs,
                     track_number, disc_number, date_added)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17)
                 ON CONFLICT(id) DO UPDATE SET
                     provider = excluded.provider,
                     title = excluded.title,
                     artist = excluded.artist,
                     album = excluded.album,
                     album_artist = excluded.album_artist,
                     duration_ms = excluded.duration_ms,
                     year = excluded.year,
                     genre = excluded.genre,
                     file_path = excluded.file_path,
                     artwork_url = excluded.artwork_url,
                     bitrate = excluded.bitrate,
                     format = excluded.format,
                     loudness_lufs = excluded.loudness_lufs,
                     track_number = excluded.track_number,
                     disc_number = excluded.disc_number",
            )?;
            for t in tracks {
                stmt.execute(params![
                    t.id,
                    t.provider,
                    t.title,
                    t.artist,
                    t.album,
                    t.album_artist,
                    t.duration_ms as i64,
                    t.year.map(|v| v as i64),
                    t.genre,
                    t.file_path,
                    t.artwork_url,
                    t.bitrate.map(|v| v as i64),
                    t.format,
                    t.loudness_lufs,
                    t.track_number.map(|v| v as i64),
                    t.disc_number.map(|v| v as i64),
                    now,
                ])?;
            }
        }
        tx.commit()?;
        Ok(tracks.len())
    }

    pub fn get_track(&self, id: &str) -> Result<Option<TrackRecord>> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        conn.query_row(
            &format!("SELECT {TRACK_COLUMNS} FROM tracks WHERE id = ?1"),
            params![id],
            track_from_row,
        )
        .optional()
    }

    /// Library query. With `query` set this runs through FTS5, otherwise it is a plain scan.
    /// `sort` is matched against a whitelist so it can be interpolated safely.
    pub fn get_tracks(
        &self,
        query: Option<&str>,
        provider: Option<&str>,
        sort: Option<&str>,
        limit: u32,
        offset: u32,
    ) -> Result<Vec<TrackRecord>> {
        let order = match sort.unwrap_or("title") {
            "title" => "t.title COLLATE NOCASE ASC",
            "artist" => {
                "t.artist COLLATE NOCASE ASC, t.album COLLATE NOCASE ASC, t.track_number ASC"
            }
            "album" => "t.album COLLATE NOCASE ASC, t.disc_number ASC, t.track_number ASC",
            "duration" => "t.duration_ms DESC",
            "format" => "t.format COLLATE NOCASE ASC, t.title COLLATE NOCASE ASC",
            "added" => "t.date_added DESC",
            _ => "t.title COLLATE NOCASE ASC",
        };

        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let provider_filter = provider.filter(|p| *p != "all");

        let mut sql = String::from("SELECT ");
        sql.push_str(
            &TRACK_COLUMNS
                .split(", ")
                .map(|c| format!("t.{}", c.trim()))
                .collect::<Vec<_>>()
                .join(", "),
        );
        sql.push_str(" FROM tracks t");

        let search = query.map(str::trim).filter(|q| !q.is_empty());
        if search.is_some() {
            sql.push_str(" JOIN tracks_fts f ON f.rowid = t.rowid");
        }

        let mut wheres = Vec::new();
        let mut bindings: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();
        if let Some(q) = search {
            wheres.push("tracks_fts MATCH ?".to_string());
            bindings.push(Box::new(fts_match_expression(q)));
        }
        if let Some(p) = provider_filter {
            wheres.push("t.provider = ?".to_string());
            bindings.push(Box::new(p.to_string()));
        }
        if !wheres.is_empty() {
            sql.push_str(" WHERE ");
            sql.push_str(&wheres.join(" AND "));
        }
        sql.push_str(&format!(" ORDER BY {order} LIMIT ? OFFSET ?"));
        bindings.push(Box::new(limit as i64));
        bindings.push(Box::new(offset as i64));

        let mut stmt = conn.prepare(&sql)?;
        let params_ref: Vec<&dyn rusqlite::ToSql> = bindings.iter().map(|b| b.as_ref()).collect();
        let rows = stmt.query_map(params_ref.as_slice(), track_from_row)?;
        rows.collect()
    }

    pub fn count_tracks(&self, provider: Option<&str>) -> Result<u32> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        match provider.filter(|p| *p != "all") {
            Some(p) => conn.query_row(
                "SELECT COUNT(*) FROM tracks WHERE provider = ?1",
                params![p],
                |r| r.get::<_, i64>(0),
            ),
            None => conn.query_row("SELECT COUNT(*) FROM tracks", [], |r| r.get::<_, i64>(0)),
        }
        .map(|c| c as u32)
    }

    pub fn get_albums(
        &self,
        provider: Option<&str>,
        query: Option<&str>,
    ) -> Result<Vec<AlbumRecord>> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let provider_filter = provider.filter(|p| *p != "all");
        let search = query.map(str::trim).filter(|q| !q.is_empty());
        let mut sql = String::from(
            "SELECT album, MAX(album_artist), MAX(year), MAX(artwork_url), COUNT(*), SUM(duration_ms), provider
             FROM tracks",
        );
        let mut wheres = Vec::new();
        let mut bindings: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();
        if let Some(q) = search {
            let escaped = q
                .replace('\\', "\\\\")
                .replace('%', "\\%")
                .replace('_', "\\_");
            let like = format!("%{escaped}%");
            wheres.push(
                "(album LIKE ? ESCAPE '\\' OR album_artist LIKE ? ESCAPE '\\' OR artist LIKE ? ESCAPE '\\')"
                    .to_string(),
            );
            bindings.push(Box::new(like.clone()));
            bindings.push(Box::new(like.clone()));
            bindings.push(Box::new(like));
        }
        if let Some(p) = provider_filter {
            wheres.push("provider = ?".to_string());
            bindings.push(Box::new(p.to_string()));
        }
        if !wheres.is_empty() {
            sql.push_str(" WHERE ");
            sql.push_str(&wheres.join(" AND "));
        }
        sql.push_str(
            " GROUP BY album, provider ORDER BY MAX(album_artist) COLLATE NOCASE, album COLLATE NOCASE",
        );

        let mut stmt = conn.prepare(&sql)?;
        let params_ref: Vec<&dyn rusqlite::ToSql> = bindings.iter().map(|b| b.as_ref()).collect();
        let rows = stmt.query_map(params_ref.as_slice(), |row| {
            Ok(AlbumRecord {
                album: row.get(0)?,
                album_artist: row.get(1)?,
                year: row.get::<_, Option<i64>>(2)?.map(|v| v as u32),
                artwork_url: row.get(3)?,
                track_count: row.get::<_, i64>(4)? as u32,
                duration_ms: row.get::<_, i64>(5)? as u64,
                provider: row.get(6)?,
            })
        })?;
        rows.collect()
    }

    pub fn get_artists(
        &self,
        provider: Option<&str>,
        query: Option<&str>,
    ) -> Result<Vec<ArtistRecord>> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let provider_filter = provider.filter(|p| *p != "all");
        let search = query.map(str::trim).filter(|q| !q.is_empty());
        let mut sql =
            String::from("SELECT artist, provider, COUNT(DISTINCT album), COUNT(*) FROM tracks");
        let mut wheres = Vec::new();
        let mut bindings: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();
        if let Some(q) = search {
            let escaped = q
                .replace('\\', "\\\\")
                .replace('%', "\\%")
                .replace('_', "\\_");
            wheres.push("artist LIKE ? ESCAPE '\\'".to_string());
            bindings.push(Box::new(format!("%{escaped}%")));
        }
        if let Some(p) = provider_filter {
            wheres.push("provider = ?".to_string());
            bindings.push(Box::new(p.to_string()));
        }
        if !wheres.is_empty() {
            sql.push_str(" WHERE ");
            sql.push_str(&wheres.join(" AND "));
        }
        sql.push_str(" GROUP BY artist, provider ORDER BY artist COLLATE NOCASE");

        let mut stmt = conn.prepare(&sql)?;
        let params_ref: Vec<&dyn rusqlite::ToSql> = bindings.iter().map(|b| b.as_ref()).collect();
        let rows = stmt.query_map(params_ref.as_slice(), |row| {
            Ok(ArtistRecord {
                artist: row.get(0)?,
                provider: row.get(1)?,
                album_count: row.get::<_, i64>(2)? as u32,
                track_count: row.get::<_, i64>(3)? as u32,
            })
        })?;
        rows.collect()
    }

    /// Drops local rows that no longer exist on disk. Used after a rescan of a folder so that
    /// deleted files do not linger in the index.
    pub fn delete_missing_local_tracks_under(&self, root: &Path) -> Result<usize> {
        let root = root
            .canonicalize()
            .map_err(|error| rusqlite::Error::ToSqlConversionFailure(Box::new(error)))?;
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let mut stmt = conn.prepare(
            "SELECT id, file_path FROM tracks WHERE provider = 'local' AND file_path IS NOT NULL",
        )?;
        let stale: Vec<String> = stmt
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })?
            .filter_map(|r| r.ok())
            .filter(|(_, path)| {
                let path = Path::new(path);
                let under_root = path.starts_with(&root)
                    || path
                        .parent()
                        .and_then(|parent| parent.canonicalize().ok())
                        .is_some_and(|parent| parent.starts_with(&root));
                under_root && !path.exists()
            })
            .map(|(id, _)| id)
            .collect();
        drop(stmt);

        for id in &stale {
            conn.execute("DELETE FROM tracks WHERE id = ?1", params![id])?;
        }
        Ok(stale.len())
    }

    pub fn get_lyrics(&self, track_id: &str) -> Result<Option<(String, Option<String>, String)>> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        conn.query_row(
            "SELECT raw_lrc, romanized_json, provider FROM lyrics_cache WHERE track_id = ?1",
            params![track_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()
    }

    pub fn put_lyrics(
        &self,
        track_id: &str,
        raw_lrc: &str,
        romanized_json: Option<&str>,
        provider: &str,
    ) -> Result<()> {
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        conn.execute(
            "INSERT INTO lyrics_cache (track_id, raw_lrc, romanized_json, provider, cached_at)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(track_id) DO UPDATE SET
                 raw_lrc = excluded.raw_lrc,
                 romanized_json = excluded.romanized_json,
                 provider = excluded.provider,
                 cached_at = excluded.cached_at",
            params![
                track_id,
                raw_lrc,
                romanized_json,
                provider,
                chrono::Utc::now().timestamp()
            ],
        )?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mem_db() -> DbState {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(SCHEMA).unwrap();
        DbState {
            conn: Mutex::new(conn),
        }
    }

    fn sample(id: &str, title: &str, artist: &str) -> TrackRecord {
        TrackRecord {
            id: id.into(),
            provider: "local".into(),
            title: title.into(),
            artist: artist.into(),
            album: "Test Album".into(),
            album_artist: None,
            duration_ms: 1000,
            year: Some(2024),
            genre: None,
            file_path: Some(format!("/music/{id}.flac")),
            artwork_url: None,
            bitrate: Some(900),
            format: Some("flac".into()),
            loudness_lufs: Some(-12.0),
            track_number: Some(1),
            disc_number: Some(1),
        }
    }

    #[test]
    fn upsert_is_idempotent_and_fts_stays_in_sync() {
        let db = mem_db();
        db.upsert_tracks(&[sample("a", "Yoru ni Kakeru", "YOASOBI")])
            .unwrap();
        db.upsert_tracks(&[sample("a", "Yoru ni Kakeru", "YOASOBI")])
            .unwrap();

        assert_eq!(
            db.count_tracks(None).unwrap(),
            1,
            "re-scan must not duplicate rows"
        );
        let hits = db.get_tracks(Some("yoru"), None, None, 10, 0).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].artist, "YOASOBI");
    }

    #[test]
    fn fts_matches_by_artist_and_survives_punctuation() {
        let db = mem_db();
        db.upsert_tracks(&[
            sample("a", "Starboy", "The Weeknd, Daft Punk"),
            sample("b", "Blue Monday", "New Order"),
        ])
        .unwrap();

        assert_eq!(
            db.get_tracks(Some("weeknd"), None, None, 10, 0)
                .unwrap()
                .len(),
            1
        );
        // Hyphens and quotes are FTS5 operators; unescaped input would return Err here.
        assert_eq!(
            db.get_tracks(Some("new-ord"), None, None, 10, 0)
                .unwrap()
                .len(),
            1
        );
        assert!(db
            .get_tracks(Some("\"unterminated"), None, None, 10, 0)
            .unwrap()
            .is_empty());
    }

    #[test]
    fn provider_filter_and_album_rollup() {
        let db = mem_db();
        let mut spotify = sample("s", "Song", "Someone");
        spotify.provider = "spotify".into();
        db.upsert_tracks(&[sample("a", "One", "Artist"), spotify])
            .unwrap();

        assert_eq!(db.count_tracks(Some("local")).unwrap(), 1);
        assert_eq!(db.count_tracks(Some("all")).unwrap(), 2);

        let albums = db.get_albums(None, None).unwrap();
        assert_eq!(albums.len(), 2);
        assert!(albums.iter().all(|album| album.track_count == 1));
        assert!(albums.iter().all(|album| album.duration_ms == 1000));
    }

    #[test]
    fn scoped_missing_file_cleanup_keeps_other_music_roots() {
        let dir = tempfile::tempdir().unwrap();
        let scanned_root = dir.path().join("scanned");
        let other_root = dir.path().join("other");
        std::fs::create_dir_all(&scanned_root).unwrap();
        std::fs::create_dir_all(&other_root).unwrap();
        let existing = scanned_root.join("existing.flac");
        std::fs::write(&existing, b"fixture").unwrap();

        let db = mem_db();
        let mut missing = sample("missing", "Missing", "Artist");
        missing.file_path = Some(scanned_root.join("missing.flac").display().to_string());
        let mut kept = sample("kept", "Kept", "Artist");
        kept.file_path = Some(existing.display().to_string());
        let mut other = sample("other", "Other", "Artist");
        other.file_path = Some(other_root.join("unmounted.flac").display().to_string());
        db.upsert_tracks(&[missing, kept, other]).unwrap();

        assert_eq!(
            db.delete_missing_local_tracks_under(&scanned_root).unwrap(),
            1
        );
        assert!(db.get_track("missing").unwrap().is_none());
        assert!(db.get_track("kept").unwrap().is_some());
        assert!(db.get_track("other").unwrap().is_some());
    }

    #[test]
    fn unknown_sort_key_falls_back_instead_of_erroring() {
        let db = mem_db();
        db.upsert_tracks(&[sample("a", "One", "Artist")]).unwrap();
        let rows = db
            .get_tracks(None, None, Some("title; DROP TABLE tracks"), 10, 0)
            .unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(db.count_tracks(None).unwrap(), 1);
    }
}
