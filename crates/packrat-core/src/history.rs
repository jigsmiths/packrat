use std::cmp::Reverse;
use std::fmt::Write as _;
use std::time::Duration;

use sha2::{Digest, Sha256};

use crate::db::{Clip, Db, Kind};
use crate::pasteboard::Image;
use crate::{Error, Result};

pub const DEFAULT_TTL: Duration = Duration::from_secs(24 * 60 * 60);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Added {
    New,
    Bumped,
}

pub struct History {
    db: Db,
    clips: Vec<Clip>,
    ttl: Option<Duration>,
}

impl History {
    pub fn open(db: Db) -> Result<Self> {
        let mut history = Self {
            clips: db.load_without_image_data()?,
            db,
            ttl: Some(DEFAULT_TTL),
        };
        history.sort();
        Ok(history)
    }

    pub fn clips(&self) -> &[Clip] {
        &self.clips
    }

    pub fn total_size(&self) -> usize {
        self.clips.iter().map(|clip| clip.size).sum()
    }

    pub fn set_ttl(&mut self, ttl: Option<Duration>) {
        self.ttl = ttl;
    }

    pub fn add(&mut self, content: &str, now_ms: i64) -> Result<Added> {
        let hash = hash_content(content);
        if self.position(&hash).is_some() {
            self.touch(&hash, now_ms)?;
            return Ok(Added::Bumped);
        }
        let clip = Clip {
            hash,
            content: content.to_owned(),
            created_at_ms: now_ms,
            last_used_at_ms: now_ms,
            pinned: false,
            kind: Kind::Text,
            size: content.len(),
        };
        self.db.insert(&clip)?;
        self.clips.push(clip);
        self.sort();
        Ok(Added::New)
    }

    pub fn add_image(&mut self, image: &Image, now_ms: i64) -> Result<Added> {
        let hash = hash_bytes(&image.png);
        if self.position(&hash).is_some() {
            self.touch(&hash, now_ms)?;
            return Ok(Added::Bumped);
        }
        let clip = Clip {
            hash,
            content: image_label(image),
            created_at_ms: now_ms,
            last_used_at_ms: now_ms,
            pinned: false,
            kind: Kind::Image,
            size: image.png.len(),
        };
        self.db.insert_image(&clip, &image.png)?;
        self.clips.push(clip);
        self.sort();
        Ok(Added::New)
    }

    pub fn image_data(&self, hash: &str) -> Result<Vec<u8>> {
        self.db.load_data(hash)
    }

    pub fn touch(&mut self, hash: &str, now_ms: i64) -> Result<()> {
        let index = self.position(hash).ok_or(Error::NotFound)?;
        let result = self.db.set_last_used_at(hash, now_ms);
        self.drop_if_missing(index, result)?;
        self.clips[index].last_used_at_ms = now_ms;
        self.sort();
        Ok(())
    }

    pub fn toggle_pin(&mut self, hash: &str) -> Result<bool> {
        let index = self.position(hash).ok_or(Error::NotFound)?;
        let pinned = !self.clips[index].pinned;
        let result = self.db.set_pinned(hash, pinned);
        self.drop_if_missing(index, result)?;
        self.clips[index].pinned = pinned;
        self.sort();
        Ok(pinned)
    }

    pub fn delete(&mut self, hash: &str) -> Result<()> {
        let index = self.position(hash).ok_or(Error::NotFound)?;
        match self.db.delete(hash) {
            Ok(()) | Err(Error::NotFound) => {}
            Err(err) => return Err(err),
        }
        self.clips.remove(index);
        Ok(())
    }

    pub fn prune_expired(&mut self, now_ms: i64) -> Result<usize> {
        let Some(ttl) = self.ttl else {
            return Ok(0);
        };
        let last_used_before_ms =
            now_ms.saturating_sub(i64::try_from(ttl.as_millis()).unwrap_or(i64::MAX));
        self.db.delete_expired(last_used_before_ms)?;
        let before = self.clips.len();
        self.clips
            .retain(|clip| clip.pinned || clip.last_used_at_ms >= last_used_before_ms);
        Ok(before - self.clips.len())
    }

    fn position(&self, hash: &str) -> Option<usize> {
        self.clips.iter().position(|clip| clip.hash == hash)
    }

    fn sort(&mut self) {
        self.clips
            .sort_by_key(|clip| (!clip.pinned, Reverse(clip.last_used_at_ms)));
    }

    fn drop_if_missing(&mut self, index: usize, result: Result<()>) -> Result<()> {
        if matches!(result, Err(Error::NotFound)) {
            self.clips.remove(index);
        }
        result
    }
}

pub fn hash_content(content: &str) -> String {
    hash_bytes(content.as_bytes())
}

pub fn hash_bytes(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut hex = String::with_capacity(digest.len() * 2);
    for byte in digest.iter() {
        write!(hex, "{byte:02x}").expect("writing to a String cannot fail");
    }
    hex
}

fn image_label(image: &Image) -> String {
    let size = format!("{}×{}", image.width, image.height);
    match &image.file_name {
        Some(name) => format!("{name} · {size}"),
        None => format!("Image {size}"),
    }
}

pub fn parse_ttl(value: &str) -> Result<Option<Duration>> {
    let value = value.trim();
    if value.is_empty() {
        return Ok(Some(DEFAULT_TTL));
    }
    if value == "0" {
        return Ok(None);
    }
    let ttl = humantime::parse_duration(value).map_err(|err| Error::InvalidTtl {
        value: value.to_owned(),
        reason: err.to_string(),
    })?;
    Ok((!ttl.is_zero()).then_some(ttl))
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use tempfile::TempDir;

    use super::*;
    use crate::db::Key;

    const KEY: Key = [7; 32];
    const HOUR: i64 = 60 * 60 * 1000;

    fn open(path: &Path) -> History {
        History::open(Db::open(path, &KEY).unwrap()).unwrap()
    }

    fn open_temp() -> (TempDir, PathBuf, History) {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("packrat.db");
        let history = open(&path);
        (dir, path, history)
    }

    fn contents(history: &History) -> Vec<&str> {
        history.clips().iter().map(|c| c.content.as_str()).collect()
    }

    #[test]
    fn hash_matches_sha256_hex() {
        assert_eq!(
            hash_content("hello"),
            "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824"
        );
    }

    #[test]
    fn new_history_is_empty() {
        let (_dir, _path, history) = open_temp();
        assert!(history.clips().is_empty());
    }

    #[test]
    fn add_stores_a_new_clip() {
        let (_dir, _path, mut history) = open_temp();
        assert_eq!(history.add("one", 100).unwrap(), Added::New);
        let clip = &history.clips()[0];
        assert_eq!(clip.content, "one");
        assert_eq!(clip.hash, hash_content("one"));
        assert_eq!(clip.created_at_ms, 100);
        assert!(!clip.pinned);
    }

    #[test]
    fn newest_clip_comes_first() {
        let (_dir, _path, mut history) = open_temp();
        history.add("old", 100).unwrap();
        history.add("new", 200).unwrap();
        assert_eq!(contents(&history), ["new", "old"]);
    }

    #[test]
    fn adding_known_content_bumps_it() {
        let (_dir, path, mut history) = open_temp();
        history.add("one", 100).unwrap();
        history.add("two", 200).unwrap();
        assert_eq!(history.add("one", 300).unwrap(), Added::Bumped);
        assert_eq!(contents(&history), ["one", "two"]);
        assert_eq!(history.clips()[0].last_used_at_ms, 300);

        drop(history);
        assert_eq!(contents(&open(&path)), ["one", "two"]);
    }

    #[test]
    fn reopening_restores_clips_and_order() {
        let (_dir, path, mut history) = open_temp();
        history.add("one", 100).unwrap();
        history.add("two", 200).unwrap();
        history.add("three", 300).unwrap();
        history.toggle_pin(&hash_content("one")).unwrap();
        drop(history);

        let reopened = open(&path);
        assert_eq!(contents(&reopened), ["one", "three", "two"]);
        assert!(reopened.clips()[0].pinned);
    }

    #[test]
    fn pinned_clips_sort_before_newer_unpinned_ones() {
        let (_dir, _path, mut history) = open_temp();
        history.add("old", 100).unwrap();
        history.add("new", 200).unwrap();
        assert!(history.toggle_pin(&hash_content("old")).unwrap());
        assert_eq!(contents(&history), ["old", "new"]);

        assert!(!history.toggle_pin(&hash_content("old")).unwrap());
        assert_eq!(contents(&history), ["new", "old"]);
    }

    #[test]
    fn touch_moves_a_clip_to_the_top() {
        let (_dir, _path, mut history) = open_temp();
        history.add("one", 100).unwrap();
        history.add("two", 200).unwrap();
        history.touch(&hash_content("one"), 300).unwrap();
        assert_eq!(contents(&history), ["one", "two"]);
    }

    #[test]
    fn using_a_clip_again_keeps_when_it_was_added() {
        let (_dir, path, mut history) = open_temp();
        history.add("copied twice", 100).unwrap();
        history.add("copied twice", 200).unwrap();
        history.add("picked", 100).unwrap();
        history.touch(&hash_content("picked"), 300).unwrap();
        for clip in history.clips() {
            assert_eq!(clip.created_at_ms, 100, "{}", clip.content);
        }

        drop(history);
        for clip in open(&path).clips() {
            assert_eq!(clip.created_at_ms, 100, "{}", clip.content);
        }
    }

    #[test]
    fn delete_removes_the_clip_everywhere() {
        let (_dir, path, mut history) = open_temp();
        history.add("one", 100).unwrap();
        history.add("two", 200).unwrap();
        history.delete(&hash_content("one")).unwrap();
        assert_eq!(contents(&history), ["two"]);

        drop(history);
        assert_eq!(contents(&open(&path)), ["two"]);
    }

    #[test]
    fn deleted_content_can_be_added_again() {
        let (_dir, _path, mut history) = open_temp();
        history.add("one", 100).unwrap();
        history.delete(&hash_content("one")).unwrap();
        assert_eq!(history.add("one", 200).unwrap(), Added::New);
    }

    #[test]
    fn unknown_hash_is_not_found() {
        let (_dir, _path, mut history) = open_temp();
        assert!(matches!(history.delete("nope"), Err(Error::NotFound)));
        assert!(matches!(history.toggle_pin("nope"), Err(Error::NotFound)));
        assert!(matches!(history.touch("nope", 1), Err(Error::NotFound)));
    }

    #[test]
    fn clip_missing_from_the_database_is_dropped() {
        let (_dir, _path, mut history) = open_temp();
        history.add("pin me", 100).unwrap();
        history.add("touch me", 200).unwrap();
        history.add("delete me", 300).unwrap();
        for content in ["pin me", "touch me", "delete me"] {
            history.db.delete(&hash_content(content)).unwrap();
        }

        let pin = history.toggle_pin(&hash_content("pin me"));
        assert!(matches!(pin, Err(Error::NotFound)));
        let touch = history.touch(&hash_content("touch me"), 400);
        assert!(matches!(touch, Err(Error::NotFound)));
        history.delete(&hash_content("delete me")).unwrap();
        assert!(history.clips().is_empty());
    }

    #[test]
    fn prune_removes_old_unpinned_clips() {
        let (_dir, path, mut history) = open_temp();
        history.add("old", 0).unwrap();
        history.add("old but pinned", 0).unwrap();
        history.toggle_pin(&hash_content("old but pinned")).unwrap();
        history.add("recent", 24 * HOUR).unwrap();

        assert_eq!(history.prune_expired(25 * HOUR).unwrap(), 1);
        assert_eq!(contents(&history), ["old but pinned", "recent"]);

        drop(history);
        assert_eq!(contents(&open(&path)), ["old but pinned", "recent"]);
    }

    #[test]
    fn prune_keeps_a_clip_exactly_at_the_ttl() {
        let (_dir, _path, mut history) = open_temp();
        history.add("edge", 0).unwrap();
        assert_eq!(history.prune_expired(24 * HOUR).unwrap(), 0);
        assert_eq!(history.prune_expired(24 * HOUR + 1).unwrap(), 1);
    }

    #[test]
    fn bumping_a_clip_restarts_its_ttl() {
        let (_dir, _path, mut history) = open_temp();
        history.add("reused", 0).unwrap();
        history.add("reused", 20 * HOUR).unwrap();
        assert_eq!(history.prune_expired(30 * HOUR).unwrap(), 0);
    }

    #[test]
    fn prune_does_nothing_when_expiry_is_off() {
        let (_dir, _path, mut history) = open_temp();
        history.set_ttl(None);
        history.add("ancient", 0).unwrap();
        assert_eq!(history.prune_expired(1000 * HOUR).unwrap(), 0);
        assert_eq!(contents(&history), ["ancient"]);
    }

    #[test]
    fn prune_uses_a_custom_ttl() {
        let (_dir, _path, mut history) = open_temp();
        history.set_ttl(Some(Duration::from_secs(60)));
        history.add("short lived", 0).unwrap();
        assert_eq!(history.prune_expired(61_000).unwrap(), 1);
    }

    fn image(png: &[u8], file_name: Option<&str>) -> Image {
        Image {
            png: png.to_vec(),
            width: 1200,
            height: 800,
            file_name: file_name.map(str::to_owned),
        }
    }

    #[test]
    fn add_image_stores_a_labelled_clip_and_its_bytes() {
        let (_dir, path, mut history) = open_temp();
        let shot = image(b"png bytes", Some("Screenshot 2026-11-17 at 10.02.png"));
        assert_eq!(history.add_image(&shot, 100).unwrap(), Added::New);
        assert_eq!(
            history.add_image(&image(b"other", None), 200).unwrap(),
            Added::New
        );

        assert_eq!(
            contents(&history),
            [
                "Image 1200×800",
                "Screenshot 2026-11-17 at 10.02.png · 1200×800"
            ]
        );
        let clip = &history.clips()[1];
        assert_eq!(clip.kind, Kind::Image);
        assert_eq!(clip.hash, hash_bytes(b"png bytes"));
        assert_eq!(history.image_data(&clip.hash).unwrap(), b"png bytes");

        drop(history);
        let reopened = open(&path);
        assert_eq!(reopened.clips()[1].kind, Kind::Image);
        assert_eq!(
            reopened.image_data(&hash_bytes(b"other")).unwrap(),
            b"other"
        );
    }

    #[test]
    fn adding_a_known_image_bumps_it() {
        let (_dir, _path, mut history) = open_temp();
        history.add_image(&image(b"png", None), 100).unwrap();
        history.add("text", 200).unwrap();
        assert_eq!(
            history.add_image(&image(b"png", None), 300).unwrap(),
            Added::Bumped
        );
        assert_eq!(contents(&history), ["Image 1200×800", "text"]);
    }

    #[test]
    fn sizes_count_text_and_image_bytes() {
        let (_dir, path, mut history) = open_temp();
        history.add("héllo", 100).unwrap();
        history.add_image(&image(b"png bytes", None), 200).unwrap();
        let sizes: Vec<_> = history.clips().iter().map(|c| c.size).collect();
        assert_eq!(sizes, [9, 6]);
        assert_eq!(history.total_size(), 15);

        drop(history);
        let mut reopened = open(&path);
        let sizes: Vec<_> = reopened.clips().iter().map(|c| c.size).collect();
        assert_eq!(sizes, [9, 6]);

        reopened.delete(&hash_bytes(b"png bytes")).unwrap();
        assert_eq!(reopened.total_size(), 6);
    }

    #[test]
    fn hash_content_is_hash_bytes_of_the_text() {
        assert_eq!(hash_content("hello"), hash_bytes(b"hello"));
    }

    #[test]
    fn parse_ttl_accepts_durations() {
        assert_eq!(parse_ttl("").unwrap(), Some(DEFAULT_TTL));
        assert_eq!(parse_ttl("  ").unwrap(), Some(DEFAULT_TTL));
        assert_eq!(parse_ttl("24h").unwrap(), Some(DEFAULT_TTL));
        assert_eq!(
            parse_ttl("90m").unwrap(),
            Some(Duration::from_secs(90 * 60))
        );
        assert_eq!(
            parse_ttl("1h30m").unwrap(),
            Some(Duration::from_secs(90 * 60))
        );
        assert_eq!(parse_ttl("0").unwrap(), None);
        assert_eq!(parse_ttl("0s").unwrap(), None);
    }

    #[test]
    fn parse_ttl_rejects_bad_values() {
        for value in ["abc", "-1h", "10", "h"] {
            assert!(
                matches!(parse_ttl(value), Err(Error::InvalidTtl { .. })),
                "{value:?} should be rejected"
            );
        }
    }
}
