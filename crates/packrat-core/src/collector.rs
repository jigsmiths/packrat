use crate::history::{Added, History};
use crate::pasteboard::{Pasteboard, MAX_IMAGE_BYTES};
use crate::Result;

const PRUNE_INTERVAL_MS: i64 = 60_000;

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Poll {
    pub added: Option<Added>,
    pub pruned: usize,
}

impl Poll {
    pub fn changed(&self) -> bool {
        self.added.is_some() || self.pruned > 0
    }
}

pub struct Collector {
    last_change_count: isize,
    last_prune_ms: Option<i64>,
    paused: bool,
}

impl Collector {
    pub fn starting_after_current(pasteboard: &impl Pasteboard) -> Self {
        Self {
            last_change_count: pasteboard.change_count(),
            last_prune_ms: None,
            paused: false,
        }
    }

    pub fn is_paused(&self) -> bool {
        self.paused
    }

    #[expect(
        clippy::fn_params_excessive_bools,
        reason = "a setter for a stored flag"
    )]
    pub fn set_paused(&mut self, paused: bool) {
        self.paused = paused;
    }

    pub fn poll(
        &mut self,
        pasteboard: &impl Pasteboard,
        history: &mut History,
        now_ms: i64,
    ) -> Result<Poll> {
        let mut poll = Poll::default();

        if self
            .last_prune_ms
            .is_none_or(|last| now_ms - last >= PRUNE_INTERVAL_MS)
        {
            self.last_prune_ms = Some(now_ms);
            poll.pruned = history.prune_expired(now_ms)?;
        }

        let change_count = pasteboard.change_count();
        if change_count == self.last_change_count {
            return Ok(poll);
        }
        if self.paused {
            self.last_change_count = change_count;
            return Ok(poll);
        }
        let Ok(snapshot) = pasteboard.snapshot() else {
            return Ok(poll);
        };
        self.last_change_count = snapshot.change_count;

        if snapshot.concealed {
            return Ok(poll);
        }
        let image = snapshot
            .image
            .filter(|image| image.png.len() <= MAX_IMAGE_BYTES);
        let text = snapshot.text.filter(|text| !text.is_empty());
        poll.added = match (snapshot.file, image, text) {
            // The text next to a copied file is only its name.
            (Some(file), _, _) => Some(history.add_file(&file, now_ms)?),
            (None, _, Some(text)) => Some(history.add(&text, now_ms)?),
            (None, Some(image), None) => Some(history.add_image(&image, now_ms)?),
            (None, None, None) => None,
        };
        Ok(poll)
    }
}

#[cfg(test)]
mod tests {
    use std::cell::{Cell, RefCell};

    use tempfile::TempDir;

    use super::*;
    use crate::db::{Db, Kind};
    use crate::history::hash_content;
    use crate::pasteboard::{ChangedDuringRead, CopiedFile, Image, Snapshot};

    const HOUR: i64 = 60 * 60 * 1000;

    #[derive(Default)]
    struct FakePasteboard {
        change_count: Cell<isize>,
        text: RefCell<Option<String>>,
        image: RefCell<Option<Image>>,
        file: RefCell<Option<CopiedFile>>,
        concealed: Cell<bool>,
        changing_mid_read: Cell<bool>,
    }

    impl FakePasteboard {
        fn copy(&self, text: &str) {
            self.change_count.set(self.change_count.get() + 1);
            *self.text.borrow_mut() = Some(text.to_owned());
            *self.image.borrow_mut() = None;
            *self.file.borrow_mut() = None;
            self.concealed.set(false);
        }

        fn copy_image(&self, image: Image, text: Option<&str>) {
            self.change_count.set(self.change_count.get() + 1);
            *self.text.borrow_mut() = text.map(str::to_owned);
            *self.image.borrow_mut() = Some(image);
            *self.file.borrow_mut() = None;
            self.concealed.set(false);
        }

        fn copy_file(&self, file: CopiedFile, name: &str) {
            self.copy(name);
            *self.file.borrow_mut() = Some(file);
        }

        fn copy_secret(&self, text: &str) {
            self.copy(text);
            self.concealed.set(true);
        }

        fn copy_non_text(&self) {
            self.change_count.set(self.change_count.get() + 1);
            *self.text.borrow_mut() = None;
            *self.image.borrow_mut() = None;
            *self.file.borrow_mut() = None;
            self.concealed.set(false);
        }
    }

    impl Pasteboard for FakePasteboard {
        fn change_count(&self) -> isize {
            self.change_count.get()
        }

        fn snapshot(&self) -> std::result::Result<Snapshot, ChangedDuringRead> {
            if self.changing_mid_read.get() {
                return Err(ChangedDuringRead);
            }
            Ok(Snapshot {
                change_count: self.change_count.get(),
                text: self.text.borrow().clone(),
                image: self.image.borrow().clone(),
                file: self.file.borrow().clone(),
                concealed: self.concealed.get(),
            })
        }
    }

    fn open_history() -> (TempDir, History) {
        let dir = TempDir::new().unwrap();
        let db = Db::open(&dir.path().join("packrat.db"), &[7; 32]).unwrap();
        (dir, History::open(db).unwrap())
    }

    fn contents(history: &History) -> Vec<&str> {
        history.clips().iter().map(|c| c.content.as_str()).collect()
    }

    #[test]
    fn contents_present_at_startup_are_not_captured() {
        let (_dir, mut history) = open_history();
        let pasteboard = FakePasteboard::default();
        pasteboard.copy("copied before launch");
        let mut collector = Collector::starting_after_current(&pasteboard);

        let poll = collector.poll(&pasteboard, &mut history, 0).unwrap();
        assert_eq!(poll.added, None);
        assert!(history.clips().is_empty());
    }

    #[test]
    fn a_new_copy_is_captured_once() {
        let (_dir, mut history) = open_history();
        let pasteboard = FakePasteboard::default();
        let mut collector = Collector::starting_after_current(&pasteboard);

        pasteboard.copy("hello");
        let first = collector.poll(&pasteboard, &mut history, 100).unwrap();
        let second = collector.poll(&pasteboard, &mut history, 200).unwrap();

        assert_eq!(first.added, Some(Added::New));
        assert!(first.changed());
        assert_eq!(second.added, None);
        assert_eq!(history.clips()[0].created_at_ms, 100);
    }

    #[test]
    fn copying_known_text_again_bumps_it() {
        let (_dir, mut history) = open_history();
        let pasteboard = FakePasteboard::default();
        let mut collector = Collector::starting_after_current(&pasteboard);

        pasteboard.copy("one");
        collector.poll(&pasteboard, &mut history, 100).unwrap();
        pasteboard.copy("two");
        collector.poll(&pasteboard, &mut history, 200).unwrap();
        pasteboard.copy("one");
        let poll = collector.poll(&pasteboard, &mut history, 300).unwrap();

        assert_eq!(poll.added, Some(Added::Bumped));
        assert_eq!(contents(&history), ["one", "two"]);
    }

    #[test]
    fn concealed_copies_are_skipped() {
        let (_dir, mut history) = open_history();
        let pasteboard = FakePasteboard::default();
        let mut collector = Collector::starting_after_current(&pasteboard);

        pasteboard.copy_secret("hunter2");
        let poll = collector.poll(&pasteboard, &mut history, 100).unwrap();
        assert_eq!(poll.added, None);
        assert!(history.clips().is_empty());

        collector.poll(&pasteboard, &mut history, 200).unwrap();
        assert!(history.clips().is_empty());
        pasteboard.copy("not a secret");
        collector.poll(&pasteboard, &mut history, 300).unwrap();
        assert_eq!(contents(&history), ["not a secret"]);
    }

    #[test]
    fn empty_and_non_text_copies_are_skipped() {
        let (_dir, mut history) = open_history();
        let pasteboard = FakePasteboard::default();
        let mut collector = Collector::starting_after_current(&pasteboard);

        pasteboard.copy("");
        collector.poll(&pasteboard, &mut history, 100).unwrap();
        pasteboard.copy_non_text();
        collector.poll(&pasteboard, &mut history, 200).unwrap();
        assert!(history.clips().is_empty());
    }

    fn image(png: Vec<u8>) -> Image {
        Image {
            png,
            width: 2,
            height: 1,
        }
    }

    fn file(name: &str, bytes: &[u8]) -> CopiedFile {
        CopiedFile {
            name: name.to_owned(),
            bytes: bytes.to_vec(),
            thumbnail: None,
        }
    }

    #[test]
    fn image_data_is_captured() {
        let (_dir, mut history) = open_history();
        let pasteboard = FakePasteboard::default();
        let mut collector = Collector::starting_after_current(&pasteboard);

        pasteboard.copy_image(image(b"png".to_vec()), None);
        let poll = collector.poll(&pasteboard, &mut history, 100).unwrap();
        assert_eq!(poll.added, Some(Added::New));
        assert_eq!(contents(&history), ["Image 2×1"]);
    }

    #[test]
    fn a_copied_file_beats_its_name() {
        let (_dir, mut history) = open_history();
        let pasteboard = FakePasteboard::default();
        let mut collector = Collector::starting_after_current(&pasteboard);

        pasteboard.copy_file(file("report.pdf", b"%PDF"), "report.pdf");
        let poll = collector.poll(&pasteboard, &mut history, 100).unwrap();
        assert_eq!(poll.added, Some(Added::New));
        assert_eq!(contents(&history), ["report.pdf"]);
        assert_eq!(history.clips()[0].kind, Kind::File);
    }

    #[test]
    fn a_copied_image_file_keeps_its_original_bytes() {
        let (_dir, mut history) = open_history();
        let pasteboard = FakePasteboard::default();
        let mut collector = Collector::starting_after_current(&pasteboard);

        pasteboard.copy_file(file("anim.gif", b"GIF89a frames"), "anim.gif");
        collector.poll(&pasteboard, &mut history, 100).unwrap();
        let clip = &history.clips()[0];
        assert_eq!(clip.kind, Kind::File);
        assert_eq!(history.data(&clip.hash).unwrap(), b"GIF89a frames");
    }

    #[test]
    fn a_file_that_could_not_be_read_falls_back_to_its_name() {
        let (_dir, mut history) = open_history();
        let pasteboard = FakePasteboard::default();
        let mut collector = Collector::starting_after_current(&pasteboard);

        pasteboard.copy("huge.zip");
        collector.poll(&pasteboard, &mut history, 100).unwrap();
        assert_eq!(contents(&history), ["huge.zip"]);
        assert_eq!(history.clips()[0].kind, Kind::Text);
    }

    #[test]
    fn text_beats_image_data() {
        let (_dir, mut history) = open_history();
        let pasteboard = FakePasteboard::default();
        let mut collector = Collector::starting_after_current(&pasteboard);

        pasteboard.copy_image(image(b"png".to_vec()), Some("caption"));
        collector.poll(&pasteboard, &mut history, 100).unwrap();
        assert_eq!(contents(&history), ["caption"]);
    }

    #[test]
    fn oversized_images_are_skipped() {
        let (_dir, mut history) = open_history();
        let pasteboard = FakePasteboard::default();
        let mut collector = Collector::starting_after_current(&pasteboard);
        let huge = vec![0; MAX_IMAGE_BYTES + 1];

        pasteboard.copy_image(image(huge), None);
        collector.poll(&pasteboard, &mut history, 100).unwrap();
        assert!(history.clips().is_empty());
    }

    #[test]
    fn concealed_images_and_files_are_skipped() {
        let (_dir, mut history) = open_history();
        let pasteboard = FakePasteboard::default();
        let mut collector = Collector::starting_after_current(&pasteboard);

        pasteboard.copy_image(image(b"png".to_vec()), None);
        pasteboard.concealed.set(true);
        collector.poll(&pasteboard, &mut history, 100).unwrap();

        pasteboard.copy_file(file("secret.key", b"key"), "secret.key");
        pasteboard.concealed.set(true);
        collector.poll(&pasteboard, &mut history, 200).unwrap();
        assert!(history.clips().is_empty());
    }

    #[test]
    fn whitespace_only_text_is_kept() {
        let (_dir, mut history) = open_history();
        let pasteboard = FakePasteboard::default();
        let mut collector = Collector::starting_after_current(&pasteboard);

        pasteboard.copy("  \n");
        collector.poll(&pasteboard, &mut history, 100).unwrap();
        assert_eq!(contents(&history), ["  \n"]);
    }

    #[test]
    fn a_read_that_races_a_change_is_retried() {
        let (_dir, mut history) = open_history();
        let pasteboard = FakePasteboard::default();
        let mut collector = Collector::starting_after_current(&pasteboard);

        pasteboard.copy("settled");
        pasteboard.changing_mid_read.set(true);
        let raced = collector.poll(&pasteboard, &mut history, 100).unwrap();
        assert_eq!(raced.added, None);

        pasteboard.changing_mid_read.set(false);
        let retried = collector.poll(&pasteboard, &mut history, 200).unwrap();
        assert_eq!(retried.added, Some(Added::New));
    }

    #[test]
    fn copies_made_while_paused_are_never_captured() {
        let (_dir, mut history) = open_history();
        let pasteboard = FakePasteboard::default();
        let mut collector = Collector::starting_after_current(&pasteboard);

        collector.set_paused(true);
        assert!(collector.is_paused());
        pasteboard.copy("private");
        collector.poll(&pasteboard, &mut history, 100).unwrap();

        collector.set_paused(false);
        collector.poll(&pasteboard, &mut history, 200).unwrap();
        assert!(history.clips().is_empty());

        pasteboard.copy("after resume");
        collector.poll(&pasteboard, &mut history, 300).unwrap();
        assert_eq!(contents(&history), ["after resume"]);
    }

    #[test]
    fn expired_clips_are_pruned_at_most_once_a_minute() {
        let (_dir, mut history) = open_history();
        let pasteboard = FakePasteboard::default();
        let mut collector = Collector::starting_after_current(&pasteboard);
        history.add("old", 0).unwrap();

        let start = 24 * HOUR;
        assert_eq!(
            collector
                .poll(&pasteboard, &mut history, start)
                .unwrap()
                .pruned,
            0
        );

        let early = collector
            .poll(&pasteboard, &mut history, start + PRUNE_INTERVAL_MS - 1)
            .unwrap();
        assert_eq!(early.pruned, 0);
        assert_eq!(contents(&history), ["old"]);

        let due = collector
            .poll(&pasteboard, &mut history, start + PRUNE_INTERVAL_MS)
            .unwrap();
        assert_eq!(due.pruned, 1);
        assert!(due.changed());
        assert!(history.clips().is_empty());
    }

    #[test]
    fn pruning_still_runs_while_paused() {
        let (_dir, mut history) = open_history();
        let pasteboard = FakePasteboard::default();
        let mut collector = Collector::starting_after_current(&pasteboard);
        history.add("old", 0).unwrap();
        history.add("pinned", 0).unwrap();
        history.toggle_pin(&hash_content("pinned")).unwrap();

        collector.set_paused(true);
        let poll = collector
            .poll(&pasteboard, &mut history, 25 * HOUR)
            .unwrap();
        assert_eq!(poll.pruned, 1);
        assert_eq!(contents(&history), ["pinned"]);
    }
}
