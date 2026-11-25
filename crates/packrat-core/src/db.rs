use std::fmt::Write as _;
use std::fs::{DirBuilder, OpenOptions};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::Path;

use rusqlite::{params, Connection, ErrorCode};

use crate::{Error, Result};

pub type Key = [u8; 32];

const VACUUM_MIN_FREE_RATIO: f64 = 0.25;
const VACUUM_MIN_FREE_PAGES: i64 = 256;

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS clips (
    hash TEXT PRIMARY KEY,
    content TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    pinned INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX IF NOT EXISTS idx_clips_created_at ON clips(created_at);
";

const MIGRATIONS: [&str; 2] = [
    "
ALTER TABLE clips ADD COLUMN kind INTEGER NOT NULL DEFAULT 0;
ALTER TABLE clips ADD COLUMN data BLOB;
",
    "
ALTER TABLE clips ADD COLUMN last_used_at INTEGER NOT NULL DEFAULT 0;
UPDATE clips SET last_used_at = created_at;
CREATE INDEX idx_clips_last_used_at ON clips(last_used_at);
",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Text = 0,
    Image = 1,
}

/// One stored clipboard entry. Times are Unix time in milliseconds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Clip {
    pub hash: String,
    pub content: String,
    /// When the content was first captured.
    pub created_at: i64,
    /// When it was last captured or picked from the popup. Expiry and order use this.
    pub last_used_at: i64,
    pub pinned: bool,
    pub kind: Kind,
    pub size: usize,
}

pub struct Db {
    conn: Connection,
}

impl Db {
    pub fn open(path: &Path, key: &Key) -> Result<Self> {
        if let Some(dir) = path.parent() {
            DirBuilder::new().recursive(true).mode(0o700).create(dir)?;
        }
        // SQLite would create the file world-readable.
        OpenOptions::new()
            .create(true)
            .append(true)
            .mode(0o600)
            .open(path)?;

        let conn = Connection::open(path)?;
        conn.execute_batch(&key_pragma(key))?;

        // The key is only checked when the first page is read.
        conn.execute_batch(SCHEMA).map_err(|err| match err {
            rusqlite::Error::SqliteFailure(e, _) if e.code == ErrorCode::NotADatabase => {
                Error::WrongKey
            }
            other => Error::Sqlite(other),
        })?;

        let db = Self { conn };
        db.migrate()?;
        let _ = db.vacuum_if_fragmented();
        Ok(db)
    }

    fn migrate(&self) -> Result<()> {
        let version = usize::try_from(self.pragma_i64("user_version")?).unwrap_or(0);
        for (index, step) in MIGRATIONS.iter().enumerate().skip(version) {
            self.conn.execute_batch(&format!(
                "BEGIN; {step} PRAGMA user_version = {}; COMMIT;",
                index + 1
            ))?;
        }
        Ok(())
    }

    fn vacuum_if_fragmented(&self) -> Result<()> {
        let free = self.pragma_i64("freelist_count")?;
        let total = self.pragma_i64("page_count")?;
        if free >= VACUUM_MIN_FREE_PAGES && free as f64 >= VACUUM_MIN_FREE_RATIO * total as f64 {
            self.conn.execute_batch("VACUUM")?;
        }
        Ok(())
    }

    fn pragma_i64(&self, name: &str) -> Result<i64> {
        Ok(self
            .conn
            .query_row(&format!("PRAGMA {name}"), [], |row| row.get(0))?)
    }

    pub fn insert(&self, clip: &Clip) -> Result<()> {
        self.insert_with_data(clip, None)
    }

    pub fn insert_image(&self, clip: &Clip, png: &[u8]) -> Result<()> {
        self.insert_with_data(clip, Some(png))
    }

    fn insert_with_data(&self, clip: &Clip, data: Option<&[u8]>) -> Result<()> {
        self.conn.execute(
            "INSERT INTO clips (hash, content, created_at, last_used_at, pinned, kind, data)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                clip.hash,
                clip.content,
                clip.created_at,
                clip.last_used_at,
                clip.pinned,
                clip.kind as i64,
                data
            ],
        )?;
        Ok(())
    }

    /// Returns all clips, least recently used first. Image bytes stay on disk, see `load_data`.
    pub fn load_all(&self) -> Result<Vec<Clip>> {
        let mut stmt = self.conn.prepare(
            "SELECT hash, content, created_at, last_used_at, pinned, kind,
                    COALESCE(length(data), length(CAST(content AS BLOB)))
             FROM clips ORDER BY last_used_at",
        )?;
        let clips = stmt
            .query_map([], |row| {
                Ok(Clip {
                    hash: row.get(0)?,
                    content: row.get(1)?,
                    created_at: row.get(2)?,
                    last_used_at: row.get(3)?,
                    pinned: row.get(4)?,
                    kind: match row.get::<_, i64>(5)? {
                        1 => Kind::Image,
                        _ => Kind::Text,
                    },
                    size: usize::try_from(row.get::<_, i64>(6)?).unwrap_or(0),
                })
            })?
            .collect::<rusqlite::Result<_>>()?;
        Ok(clips)
    }

    pub fn load_data(&self, hash: &str) -> Result<Vec<u8>> {
        let data: Option<Vec<u8>> = self
            .conn
            .query_row("SELECT data FROM clips WHERE hash = ?1", [hash], |row| {
                row.get(0)
            })
            .map_err(|err| match err {
                rusqlite::Error::QueryReturnedNoRows => Error::NotFound,
                other => Error::Sqlite(other),
            })?;
        data.ok_or(Error::NotFound)
    }

    pub fn delete(&self, hash: &str) -> Result<()> {
        let changed = self
            .conn
            .execute("DELETE FROM clips WHERE hash = ?1", [hash])?;
        found(changed)
    }

    pub fn set_pinned(&self, hash: &str, pinned: bool) -> Result<()> {
        let changed = self.conn.execute(
            "UPDATE clips SET pinned = ?1 WHERE hash = ?2",
            params![pinned, hash],
        )?;
        found(changed)
    }

    pub fn set_last_used_at(&self, hash: &str, last_used_at: i64) -> Result<()> {
        let changed = self.conn.execute(
            "UPDATE clips SET last_used_at = ?1 WHERE hash = ?2",
            params![last_used_at, hash],
        )?;
        found(changed)
    }

    /// Deletes unpinned clips last used before `cutoff` and returns how many were removed.
    pub fn delete_expired(&self, cutoff: i64) -> Result<usize> {
        Ok(self.conn.execute(
            "DELETE FROM clips WHERE pinned = 0 AND last_used_at < ?1",
            [cutoff],
        )?)
    }
}

fn key_pragma(key: &Key) -> String {
    let mut pragma = String::from("PRAGMA key = \"x'");
    for byte in key {
        write!(pragma, "{byte:02x}").expect("writing to a String cannot fail");
    }
    pragma.push_str("'\";");
    pragma
}

fn found(rows_changed: usize) -> Result<()> {
    if rows_changed == 0 {
        Err(Error::NotFound)
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use tempfile::TempDir;

    use super::*;

    const KEY: Key = [7; 32];

    fn clip(hash: &str, content: &str, last_used_at: i64) -> Clip {
        Clip {
            hash: hash.into(),
            content: content.into(),
            created_at: 0,
            last_used_at,
            pinned: false,
            kind: Kind::Text,
            size: content.len(),
        }
    }

    fn image(hash: &str, created_at: i64, size: usize) -> Clip {
        Clip {
            kind: Kind::Image,
            size,
            ..clip(hash, "Image 1×1", created_at)
        }
    }

    fn open_temp() -> (TempDir, Db) {
        let dir = TempDir::new().unwrap();
        let db = Db::open(&dir.path().join("packrat.db"), &KEY).unwrap();
        (dir, db)
    }

    #[test]
    fn new_database_is_empty() {
        let (_dir, db) = open_temp();
        assert!(db.load_all().unwrap().is_empty());
    }

    #[test]
    fn open_creates_missing_directory() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("nested/deeper/packrat.db");
        Db::open(&path, &KEY).unwrap();
        assert!(path.exists());
    }

    #[test]
    fn database_file_is_private() {
        use std::os::unix::fs::PermissionsExt;

        let dir = TempDir::new().unwrap();
        let path = dir.path().join("packrat.db");
        Db::open(&path, &KEY).unwrap();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
    }

    #[test]
    fn reopen_keeps_rows() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("packrat.db");
        Db::open(&path, &KEY)
            .unwrap()
            .insert(&clip("h1", "one", 1))
            .unwrap();
        let reopened = Db::open(&path, &KEY).unwrap();
        assert_eq!(reopened.load_all().unwrap(), vec![clip("h1", "one", 1)]);
    }

    #[test]
    fn wrong_key_is_rejected() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("packrat.db");
        Db::open(&path, &KEY)
            .unwrap()
            .insert(&clip("h1", "one", 1))
            .unwrap();
        let result = Db::open(&path, &[8; 32]);
        assert!(matches!(result, Err(Error::WrongKey)));
    }

    #[test]
    fn file_holds_no_plaintext() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("packrat.db");
        let marker = "PLAINTEXT-MARKER-12345";
        Db::open(&path, &KEY)
            .unwrap()
            .insert(&clip("marker-hash-67890", marker, 1))
            .unwrap();

        let bytes = std::fs::read(&path).unwrap();
        let contains = |needle: &str| bytes.windows(needle.len()).any(|w| w == needle.as_bytes());
        assert!(!bytes.starts_with(b"SQLite format 3"));
        assert!(!contains(marker));
        assert!(!contains("marker-hash-67890"));
    }

    #[test]
    fn load_all_returns_oldest_first() {
        let (_dir, db) = open_temp();
        db.insert(&clip("h2", "second", 200)).unwrap();
        db.insert(&clip("h1", "first", 100)).unwrap();
        let hashes: Vec<_> = db.load_all().unwrap().into_iter().map(|c| c.hash).collect();
        assert_eq!(hashes, ["h1", "h2"]);
    }

    #[test]
    fn round_trip_keeps_content_exactly() {
        let (_dir, db) = open_temp();
        let content = "tabs\tnewlines\n quotes \"'` unicode 日本語 🦀 and a nul-free tail";
        let mut pinned = clip("h1", content, 1_700_000_000_123);
        pinned.pinned = true;
        db.insert(&pinned).unwrap();
        assert_eq!(db.load_all().unwrap(), vec![pinned]);
    }

    #[test]
    fn new_database_is_at_the_latest_version() {
        let (_dir, db) = open_temp();
        assert_eq!(
            db.pragma_i64("user_version").unwrap(),
            MIGRATIONS.len() as i64
        );
    }

    #[test]
    fn version_0_database_is_migrated_with_its_rows() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("packrat.db");
        {
            let conn = Connection::open(&path).unwrap();
            conn.execute_batch(&key_pragma(&KEY)).unwrap();
            conn.execute_batch(SCHEMA).unwrap();
            conn.execute(
                "INSERT INTO clips (hash, content, created_at, pinned) VALUES ('h1', 'one', 1, 1)",
                [],
            )
            .unwrap();
        }

        let db = Db::open(&path, &KEY).unwrap();
        let mut pinned = clip("h1", "one", 1);
        pinned.created_at = 1;
        pinned.pinned = true;
        assert_eq!(db.load_all().unwrap(), vec![pinned]);
        assert_eq!(
            db.pragma_i64("user_version").unwrap(),
            MIGRATIONS.len() as i64
        );

        drop(db);
        Db::open(&path, &KEY).unwrap();
    }

    #[test]
    fn image_bytes_round_trip_but_stay_out_of_load_all() {
        let (_dir, db) = open_temp();
        let png = vec![0x89, b'P', b'N', b'G', 0, 1, 2, 255];
        db.insert_image(&image("img", 1, png.len()), &png).unwrap();
        db.insert(&clip("txt", "text", 2)).unwrap();

        assert_eq!(
            db.load_all().unwrap(),
            vec![image("img", 1, png.len()), clip("txt", "text", 2)]
        );
        assert_eq!(db.load_data("img").unwrap(), png);
    }

    #[test]
    fn text_and_missing_clips_have_no_data() {
        let (_dir, db) = open_temp();
        db.insert(&clip("txt", "text", 1)).unwrap();
        assert!(matches!(db.load_data("txt"), Err(Error::NotFound)));
        assert!(matches!(db.load_data("nope"), Err(Error::NotFound)));
    }

    #[test]
    fn duplicate_hash_is_rejected() {
        let (_dir, db) = open_temp();
        db.insert(&clip("h1", "one", 1)).unwrap();
        assert!(db.insert(&clip("h1", "again", 2)).is_err());
    }

    #[test]
    fn delete_removes_only_that_row() {
        let (_dir, db) = open_temp();
        db.insert(&clip("h1", "one", 1)).unwrap();
        db.insert(&clip("h2", "two", 2)).unwrap();
        db.delete("h1").unwrap();
        assert_eq!(db.load_all().unwrap(), vec![clip("h2", "two", 2)]);
    }

    #[test]
    fn missing_hash_is_not_found() {
        let (_dir, db) = open_temp();
        assert!(matches!(db.delete("nope"), Err(Error::NotFound)));
        assert!(matches!(db.set_pinned("nope", true), Err(Error::NotFound)));
        assert!(matches!(
            db.set_last_used_at("nope", 1),
            Err(Error::NotFound)
        ));
    }

    #[test]
    fn set_pinned_toggles_both_ways() {
        let (_dir, db) = open_temp();
        db.insert(&clip("h1", "one", 1)).unwrap();
        db.set_pinned("h1", true).unwrap();
        assert!(db.load_all().unwrap()[0].pinned);
        db.set_pinned("h1", false).unwrap();
        assert!(!db.load_all().unwrap()[0].pinned);
    }

    #[test]
    fn set_last_used_at_leaves_created_at_alone() {
        let (_dir, db) = open_temp();
        let mut added = clip("h1", "one", 1);
        added.created_at = 1;
        db.insert(&added).unwrap();
        db.set_last_used_at("h1", 99).unwrap();
        let loaded = &db.load_all().unwrap()[0];
        assert_eq!((loaded.created_at, loaded.last_used_at), (1, 99));
    }

    #[test]
    fn delete_expired_goes_by_last_use() {
        let (_dir, db) = open_temp();
        let mut old_but_used = clip("h1", "one", 900);
        old_but_used.created_at = 100;
        db.insert(&old_but_used).unwrap();
        assert_eq!(db.delete_expired(500).unwrap(), 0);
    }

    #[test]
    fn delete_expired_spares_pinned_and_recent() {
        let (_dir, db) = open_temp();
        db.insert(&clip("old", "old", 100)).unwrap();
        db.insert(&clip("old-pinned", "old pinned", 100)).unwrap();
        db.set_pinned("old-pinned", true).unwrap();
        db.insert(&clip("edge", "exactly at cutoff", 500)).unwrap();
        db.insert(&clip("new", "new", 900)).unwrap();

        assert_eq!(db.delete_expired(500).unwrap(), 1);
        let hashes: Vec<_> = db.load_all().unwrap().into_iter().map(|c| c.hash).collect();
        assert_eq!(hashes, ["old-pinned", "edge", "new"]);
    }

    #[test]
    fn version_1_database_gains_last_used_at_from_created_at() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("packrat.db");
        {
            let conn = Connection::open(&path).unwrap();
            conn.execute_batch(&key_pragma(&KEY)).unwrap();
            conn.execute_batch(SCHEMA).unwrap();
            conn.execute_batch(
                "ALTER TABLE clips ADD COLUMN kind INTEGER NOT NULL DEFAULT 0;
                 ALTER TABLE clips ADD COLUMN data BLOB;
                 PRAGMA user_version = 1;
                 INSERT INTO clips (hash, content, created_at, pinned) VALUES ('h1', 'one', 123, 1);",
            )
            .unwrap();
        }

        let db = Db::open(&path, &KEY).unwrap();
        let loaded = &db.load_all().unwrap()[0];
        assert_eq!((loaded.created_at, loaded.last_used_at), (123, 123));
        assert!(loaded.pinned);
    }

    fn fill(db: &Db, rows: usize) {
        let content = "x".repeat(4096);
        for i in 0..rows {
            db.insert(&clip(&format!("h{i}"), &content, i as i64))
                .unwrap();
        }
    }

    #[test]
    fn open_vacuums_a_fragmented_database() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("packrat.db");
        let db = Db::open(&path, &KEY).unwrap();
        fill(&db, 600);
        db.delete_expired(590).unwrap();
        assert!(db.pragma_i64("freelist_count").unwrap() >= VACUUM_MIN_FREE_PAGES);
        drop(db);

        let reopened = Db::open(&path, &KEY).unwrap();
        assert_eq!(reopened.pragma_i64("freelist_count").unwrap(), 0);
        assert_eq!(reopened.load_all().unwrap().len(), 10);
    }

    #[test]
    fn open_skips_vacuum_below_threshold() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("packrat.db");
        let db = Db::open(&path, &KEY).unwrap();
        fill(&db, 40);
        db.delete_expired(20).unwrap();
        let free = db.pragma_i64("freelist_count").unwrap();
        assert!(free > 0 && free < VACUUM_MIN_FREE_PAGES);
        drop(db);

        let reopened = Db::open(&path, &KEY).unwrap();
        assert_eq!(reopened.pragma_i64("freelist_count").unwrap(), free);
    }
}
