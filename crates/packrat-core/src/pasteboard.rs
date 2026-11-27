use std::fs::{DirBuilder, OpenOptions};
use std::io::{self, Cursor, Write as _};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use image::ImageFormat;
use objc2::rc::autoreleasepool;
use objc2_app_kit::{NSBitmapImageRep, NSPasteboard};
use objc2_foundation::{NSData, NSString, NSURL};

pub const MAX_IMAGE_BYTES: usize = 25 * 1024 * 1024;
pub const MAX_FILE_BYTES: usize = 25 * 1024 * 1024;

const QLMANAGE: &str = "/usr/bin/qlmanage";
const THUMBNAIL_PIXELS: &str = "1024";
const THUMBNAIL_TIMEOUT: Duration = Duration::from_secs(2);
const THUMBNAIL_POLL: Duration = Duration::from_millis(20);

// Types that password managers attach to secrets, see nspasteboard.org.
const CONCEALED_TYPES: [&str; 2] = [
    "org.nspasteboard.ConcealedType",
    "org.nspasteboard.TransientType",
];

// The values of AppKit's NSPasteboardType constants, which are unsafe to read.
const TEXT_TYPE: &str = "public.utf8-plain-text";
const FILE_URL_TYPE: &str = "public.file-url";
const PNG_TYPE: &str = "public.png";
const TIFF_TYPE: &str = "public.tiff";
const PDF_TYPE: &str = "com.adobe.pdf";

const IMAGE_EXTENSIONS: [&str; 10] = [
    "png", "jpg", "jpeg", "heic", "heif", "gif", "tif", "tiff", "bmp", "webp",
];

const PNG_SIGNATURE: &[u8] = b"\x89PNG\r\n\x1a\n";

#[derive(Clone, PartialEq, Eq)]
pub struct Image {
    pub png: Vec<u8>,
    pub width: usize,
    pub height: usize,
}

impl std::fmt::Debug for Image {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Image")
            .field("bytes", &self.png.len())
            .field("width", &self.width)
            .field("height", &self.height)
            .finish()
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct CopiedFile {
    pub name: String,
    pub bytes: Vec<u8>,
    pub thumbnail: Option<Vec<u8>>,
}

impl std::fmt::Debug for CopiedFile {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CopiedFile")
            .field("name", &self.name)
            .field("bytes", &self.bytes.len())
            .field("thumbnail", &self.thumbnail.as_ref().map(Vec::len))
            .finish()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Snapshot {
    pub change_count: isize,
    pub text: Option<String>,
    pub image: Option<Image>,
    pub file: Option<CopiedFile>,
    pub concealed: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Content {
    Image,
    Pdf,
    FileOnly,
}

impl Content {
    fn of(path: &Path) -> Self {
        let extension = path
            .extension()
            .and_then(|ext| ext.to_str())
            .map(str::to_ascii_lowercase);
        match extension.as_deref() {
            Some("pdf") => Self::Pdf,
            Some(ext) if IMAGE_EXTENSIONS.contains(&ext) => Self::Image,
            Some(_) | None => Self::FileOnly,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChangedDuringRead;

pub trait Pasteboard {
    fn change_count(&self) -> isize;

    fn snapshot(&self) -> Result<Snapshot, ChangedDuringRead>;
}

pub struct SystemPasteboard;

impl Pasteboard for SystemPasteboard {
    fn change_count(&self) -> isize {
        NSPasteboard::generalPasteboard().changeCount()
    }

    fn snapshot(&self) -> Result<Snapshot, ChangedDuringRead> {
        autoreleasepool(|_| {
            let pasteboard = NSPasteboard::generalPasteboard();
            let change_count = pasteboard.changeCount();
            let types: Vec<String> = pasteboard
                .types()
                .map(|types| types.iter().map(|name| name.to_string()).collect())
                .unwrap_or_default();
            let text = pasteboard
                .stringForType(&NSString::from_str(TEXT_TYPE))
                .map(|text| text.to_string());
            let concealed = is_concealed(&types);
            let has_file = types.iter().any(|name| name == FILE_URL_TYPE);
            let file = if has_file && !concealed {
                copied_file(&pasteboard).and_then(|path| {
                    let mut file = read_file(&path)?;
                    file.thumbnail = quicklook_thumbnail(&path);
                    Some(file)
                })
            } else {
                None
            };
            // Image data next to a file is only its Finder icon.
            let image = if concealed || has_file || text.is_some() {
                None
            } else {
                image_data(&pasteboard)
            };

            if pasteboard.changeCount() != change_count {
                return Err(ChangedDuringRead);
            }
            Ok(Snapshot {
                change_count,
                text,
                image,
                file,
                concealed,
            })
        })
    }
}

fn copied_file(pasteboard: &NSPasteboard) -> Option<PathBuf> {
    if pasteboard.pasteboardItems()?.count() != 1 {
        return None;
    }
    let url = pasteboard.stringForType(&NSString::from_str(FILE_URL_TYPE))?;
    let url = NSURL::URLWithString(&url)?;
    // Some file managers copy file reference URLs such as file:///.file/id=...
    let url = if url.isFileReferenceURL() {
        url.filePathURL()?
    } else {
        url
    };
    url.isFileURL()
        .then(|| url.path())
        .flatten()
        .map(|path| PathBuf::from(path.to_string()))
}

fn read_file(path: &Path) -> Option<CopiedFile> {
    let metadata = std::fs::metadata(path).ok()?;
    if !metadata.is_file() || metadata.len() > MAX_FILE_BYTES as u64 {
        return None;
    }
    Some(CopiedFile {
        name: path.file_name()?.to_string_lossy().into_owned(),
        bytes: std::fs::read(path).ok()?,
        thumbnail: None,
    })
}

// QuickLook's own API is unsafe in objc2, so this runs the command line tool that ships with macOS.
fn quicklook_thumbnail(path: &Path) -> Option<Vec<u8>> {
    let since_epoch = SystemTime::now().duration_since(UNIX_EPOCH).ok()?;
    let out = std::env::temp_dir().join(format!(
        "packrat-thumbnail-{}-{}",
        std::process::id(),
        since_epoch.as_nanos()
    ));
    DirBuilder::new().mode(0o700).create(&out).ok()?;

    let spawned = Command::new(QLMANAGE)
        .args(["-t", "-s", THUMBNAIL_PIXELS, "-o"])
        .arg(&out)
        .arg(path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
    let thumbnail = match spawned {
        Ok(mut child) => finished_in_time(&mut child)
            .then(|| first_png_in(&out))
            .flatten(),
        Err(err) => {
            log::warn!("could not run qlmanage: {err}");
            None
        }
    };
    if let Err(err) = std::fs::remove_dir_all(&out) {
        log::warn!("could not remove the thumbnail folder: {err}");
    }
    thumbnail
}

fn finished_in_time(child: &mut Child) -> bool {
    let deadline = Instant::now() + THUMBNAIL_TIMEOUT;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return status.success(),
            Ok(None) if Instant::now() < deadline => thread::sleep(THUMBNAIL_POLL),
            Ok(None) | Err(_) => {
                if let Err(err) = child.kill().and_then(|()| child.wait()) {
                    log::warn!("could not stop qlmanage: {err}");
                }
                return false;
            }
        }
    }
}

fn first_png_in(dir: &Path) -> Option<Vec<u8>> {
    let entry = std::fs::read_dir(dir).ok()?.next()?.ok()?;
    let bytes = std::fs::read(entry.path()).ok()?;
    png_size(&bytes).is_some().then_some(bytes)
}

pub fn clear_staged(dir: &Path) -> io::Result<()> {
    match std::fs::remove_dir_all(dir) {
        Err(err) if err.kind() != io::ErrorKind::NotFound => Err(err),
        Ok(()) | Err(_) => Ok(()),
    }
}

pub fn replace_staged_file(dir: &Path, name: &str, bytes: &[u8]) -> io::Result<PathBuf> {
    let name = Path::new(name)
        .file_name()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "not a file name"))?;
    clear_staged(dir)?;
    DirBuilder::new().recursive(true).mode(0o700).create(dir)?;
    let path = dir.join(name);
    OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&path)?
        .write_all(bytes)?;
    Ok(path)
}

fn image_data(pasteboard: &NSPasteboard) -> Option<Image> {
    let data = pasteboard
        .dataForType(&NSString::from_str(PNG_TYPE))
        .or_else(|| pasteboard.dataForType(&NSString::from_str(TIFF_TYPE)))?;
    to_png(data.to_vec())
}

fn to_png(bytes: Vec<u8>) -> Option<Image> {
    if let Some((width, height)) = png_size(&bytes) {
        return Some(Image {
            png: bytes,
            width,
            height,
        });
    }
    // AppKit decodes formats the image crate cannot, such as HEIC, and its PNG encoder is unsafe in objc2.
    let tiff = NSBitmapImageRep::imageRepWithData(&NSData::with_bytes(&bytes))?
        .TIFFRepresentation()?
        .to_vec();
    let decoded = image::load_from_memory_with_format(&tiff, ImageFormat::Tiff).ok()?;
    let mut png = Vec::new();
    decoded
        .write_to(&mut Cursor::new(&mut png), ImageFormat::Png)
        .ok()?;
    Some(Image {
        png,
        width: usize::try_from(decoded.width()).ok()?,
        height: usize::try_from(decoded.height()).ok()?,
    })
}

fn png_size(bytes: &[u8]) -> Option<(usize, usize)> {
    let header = bytes.strip_prefix(PNG_SIGNATURE)?;
    if header.get(4..8)? != b"IHDR" {
        return None;
    }
    let read = |range: std::ops::Range<usize>| -> Option<usize> {
        let be: [u8; 4] = header.get(range)?.try_into().ok()?;
        usize::try_from(u32::from_be_bytes(be)).ok()
    };
    Some((read(8..12)?, read(12..16)?))
}

impl SystemPasteboard {
    pub fn write_text(&self, text: &str) -> bool {
        autoreleasepool(|_| {
            let pasteboard = NSPasteboard::generalPasteboard();
            pasteboard.clearContents();
            pasteboard.setString_forType(&NSString::from_str(text), &NSString::from_str(TEXT_TYPE))
        })
    }

    pub fn write_image(&self, png: &[u8]) -> bool {
        autoreleasepool(|_| {
            let pasteboard = NSPasteboard::generalPasteboard();
            pasteboard.clearContents();
            pasteboard.setData_forType(
                Some(&NSData::with_bytes(png)),
                &NSString::from_str(PNG_TYPE),
            )
        })
    }

    pub fn write_file(&self, path: &Path, bytes: &[u8]) -> bool {
        let Some(path_text) = path.to_str() else {
            return false;
        };
        autoreleasepool(|_| {
            let Some(url) = NSURL::fileURLWithPath(&NSString::from_str(path_text)).absoluteString()
            else {
                return false;
            };
            let content = match Content::of(path) {
                Content::Image => to_png(bytes.to_vec()).map(|image| (PNG_TYPE, image.png)),
                Content::Pdf => Some((PDF_TYPE, bytes.to_vec())),
                Content::FileOnly => None,
            };
            let pasteboard = NSPasteboard::generalPasteboard();
            pasteboard.clearContents();
            let written = pasteboard.setString_forType(&url, &NSString::from_str(FILE_URL_TYPE));
            if let Some((content_type, data)) = content {
                if !pasteboard.setData_forType(
                    Some(&NSData::with_bytes(&data)),
                    &NSString::from_str(content_type),
                ) {
                    log::warn!("could not add {content_type} next to the file");
                }
            }
            written
        })
    }
}

fn is_concealed(types: &[String]) -> bool {
    types
        .iter()
        .any(|name| CONCEALED_TYPES.contains(&name.as_str()))
}

#[cfg(test)]
mod tests {
    use std::sync::{Mutex, MutexGuard, PoisonError};

    use super::*;

    // AppKit aborts the test process when tests use the pasteboard at the same time.
    static SYSTEM_PASTEBOARD: Mutex<()> = Mutex::new(());

    fn lock_system_pasteboard() -> MutexGuard<'static, ()> {
        SYSTEM_PASTEBOARD
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    fn types(names: &[&str]) -> Vec<String> {
        names.iter().map(|name| (*name).to_owned()).collect()
    }

    #[test]
    fn plain_text_is_not_concealed() {
        assert!(!is_concealed(&types(&[
            "public.utf8-plain-text",
            "NSStringPboardType"
        ])));
        assert!(!is_concealed(&[]));
    }

    #[test]
    fn password_manager_markers_are_concealed() {
        for marker in CONCEALED_TYPES {
            assert!(
                is_concealed(&types(&["public.utf8-plain-text", marker])),
                "{marker}"
            );
        }
    }

    #[test]
    fn other_nspasteboard_markers_are_not_concealed() {
        assert!(!is_concealed(&types(&[
            "org.nspasteboard.AutoGeneratedType",
            "org.nspasteboard.source"
        ])));
    }

    #[test]
    fn marker_names_must_match_exactly() {
        assert!(!is_concealed(&types(&[
            "org.nspasteboard.concealedtype",
            "org.nspasteboard.ConcealedType.extra"
        ])));
    }

    const TINY_PNG: &[u8] = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR\0\0\0\x01\0\0\0\x01\x08\x06\0\0\0\x1f\x15\xc4\x89\0\0\0\nIDATx\x9cc\0\x01\0\0\x05\0\x01\r\n-\xb4\0\0\0\0IEND\xaeB`\x82";

    fn tiny_bmp() -> Vec<u8> {
        let mut bmp = Vec::new();
        bmp.extend_from_slice(b"BM");
        bmp.extend_from_slice(&62u32.to_le_bytes());
        bmp.extend_from_slice(&0u32.to_le_bytes());
        bmp.extend_from_slice(&54u32.to_le_bytes());
        bmp.extend_from_slice(&40u32.to_le_bytes());
        bmp.extend_from_slice(&2i32.to_le_bytes());
        bmp.extend_from_slice(&1i32.to_le_bytes());
        bmp.extend_from_slice(&1u16.to_le_bytes());
        bmp.extend_from_slice(&24u16.to_le_bytes());
        bmp.extend_from_slice(&0u32.to_le_bytes());
        bmp.extend_from_slice(&8u32.to_le_bytes());
        bmp.extend_from_slice(&2835u32.to_le_bytes());
        bmp.extend_from_slice(&2835u32.to_le_bytes());
        bmp.extend_from_slice(&0u32.to_le_bytes());
        bmp.extend_from_slice(&0u32.to_le_bytes());
        bmp.extend_from_slice(&[255, 0, 0, 0, 0, 255, 0, 0]);
        bmp
    }

    #[test]
    fn png_size_reads_the_header() {
        assert_eq!(png_size(TINY_PNG), Some((1, 1)));
        assert_eq!(png_size(b"\x89PNG\r\n\x1a\n"), None);
        assert_eq!(png_size(b"GIF89a"), None);
        assert_eq!(png_size(&[]), None);
    }

    #[test]
    fn png_bytes_are_kept_as_they_are() {
        let image = to_png(TINY_PNG.to_vec()).unwrap();
        assert_eq!(image.png, TINY_PNG);
        assert_eq!((image.width, image.height), (1, 1));
    }

    #[test]
    fn other_formats_are_converted_to_png() {
        let image = to_png(tiny_bmp()).unwrap();
        assert_eq!(png_size(&image.png), Some((2, 1)));
        assert_eq!((image.width, image.height), (2, 1));
    }

    #[test]
    fn heic_is_converted_to_png() {
        let image = to_png(include_bytes!("../testdata/tiny.heic").to_vec()).unwrap();
        assert_eq!(png_size(&image.png), Some((4, 3)));
        assert_eq!((image.width, image.height), (4, 3));
    }

    #[test]
    fn bytes_that_are_not_an_image_are_rejected() {
        assert!(to_png(b"just some text".to_vec()).is_none());
    }

    #[test]
    fn image_files_are_recognized_by_extension() {
        for name in [
            "a.png",
            "b.JPG",
            "Screenshot 2026-11-17 at 10.02.png",
            "c.heic",
        ] {
            assert_eq!(Content::of(Path::new(name)), Content::Image, "{name}");
        }
        for name in ["notes.txt", "archive.png.zip", "png", "noext"] {
            assert_eq!(Content::of(Path::new(name)), Content::FileOnly, "{name}");
        }
    }

    #[test]
    fn copied_files_keep_their_name_and_original_bytes() {
        let dir = tempfile::TempDir::new().unwrap();
        for (name, bytes) in [
            ("Q3 report.pdf", b"%PDF-1.7 not really".as_slice()),
            ("photo.jpg", b"\xff\xd8\xff not a png".as_slice()),
            ("noext", b"".as_slice()),
        ] {
            let path = dir.path().join(name);
            std::fs::write(&path, bytes).unwrap();
            let file = read_file(&path).unwrap();
            assert_eq!(file.name, name);
            assert_eq!(file.bytes, bytes);
            assert_eq!(file.thumbnail, None);
        }
    }

    #[test]
    fn unusable_files_are_not_read() {
        let dir = tempfile::TempDir::new().unwrap();
        assert!(read_file(&dir.path().join("missing.png")).is_none());
        assert!(read_file(dir.path()).is_none());

        let huge = dir.path().join("huge.zip");
        let file = std::fs::File::create(&huge).unwrap();
        file.set_len(MAX_FILE_BYTES as u64 + 1).unwrap();
        assert!(read_file(&huge).is_none());

        file.set_len(MAX_FILE_BYTES as u64).unwrap();
        assert!(read_file(&huge).is_some());
    }

    #[test]
    fn staged_files_are_private_and_replace_the_previous_one() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::TempDir::new().unwrap();
        let staging = dir.path().join("cache/pasted");

        let first = replace_staged_file(&staging, "a.txt", b"one").unwrap();
        assert_eq!(first, staging.join("a.txt"));
        assert_eq!(std::fs::read(&first).unwrap(), b"one");
        let mode = |path: &Path| std::fs::metadata(path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&staging), 0o700);
        assert_eq!(mode(&first), 0o600);

        let second = replace_staged_file(&staging, "b.pdf", b"two").unwrap();
        assert!(!first.exists());
        assert_eq!(std::fs::read(&second).unwrap(), b"two");

        clear_staged(&staging).unwrap();
        assert!(!staging.exists());
        clear_staged(&staging).unwrap();
    }

    #[test]
    fn staged_names_cannot_leave_the_staging_folder() {
        let dir = tempfile::TempDir::new().unwrap();
        let staging = dir.path().join("pasted");

        let escaped = replace_staged_file(&staging, "../../evil.txt", b"x").unwrap();
        assert_eq!(escaped, staging.join("evil.txt"));
        assert!(!dir.path().join("evil.txt").exists());

        let absolute = replace_staged_file(&staging, "/etc/hosts", b"x").unwrap();
        assert_eq!(absolute, staging.join("hosts"));

        for name in ["..", "", "/"] {
            assert!(
                replace_staged_file(&staging, name, b"x").is_err(),
                "{name:?}"
            );
        }
    }

    #[test]
    fn images_and_pdfs_get_a_content_type_next_to_the_file() {
        assert_eq!(Content::of(Path::new("a.PNG")), Content::Image);
        assert_eq!(Content::of(Path::new("photo.heic")), Content::Image);
        assert_eq!(Content::of(Path::new("doc.Pdf")), Content::Pdf);
        assert_eq!(Content::of(Path::new("notes.txt")), Content::FileOnly);
        assert_eq!(Content::of(Path::new("pdf")), Content::FileOnly);
    }

    #[test]
    fn a_thumbnail_that_is_not_a_png_is_rejected() {
        let dir = tempfile::TempDir::new().unwrap();
        assert_eq!(first_png_in(dir.path()), None);

        std::fs::write(dir.path().join("a.txt.png"), b"not a png").unwrap();
        assert_eq!(first_png_in(dir.path()), None);

        std::fs::write(dir.path().join("a.txt.png"), TINY_PNG).unwrap();
        assert_eq!(first_png_in(dir.path()).as_deref(), Some(TINY_PNG));
    }

    #[test]
    #[ignore = "runs the system qlmanage, run by hand with --ignored"]
    fn quicklook_makes_a_png_thumbnail() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("notes.txt");
        std::fs::write(&path, "hello packrat").unwrap();
        let thumbnail = quicklook_thumbnail(&path).expect("no thumbnail");
        assert!(png_size(&thumbnail).is_some());
    }

    #[test]
    #[ignore = "overwrites the real pasteboard, run by hand with --ignored"]
    fn system_file_write_is_visible_in_the_next_snapshot() {
        let _lock = lock_system_pasteboard();
        let dir = tempfile::TempDir::new().unwrap();
        let path = replace_staged_file(dir.path(), "shot.png", TINY_PNG).unwrap();
        assert!(SystemPasteboard.write_file(&path, TINY_PNG));

        let snapshot = SystemPasteboard
            .snapshot()
            .expect("pasteboard changed mid-read");
        assert_eq!(snapshot.image, None);
        let file = snapshot.file.expect("no file on the pasteboard");
        assert_eq!(file.name, "shot.png");
        assert_eq!(file.bytes, TINY_PNG);

        let pasteboard = NSPasteboard::generalPasteboard();
        let png = pasteboard
            .dataForType(&NSString::from_str(PNG_TYPE))
            .expect("no PNG next to the file");
        assert_eq!(png.to_vec(), TINY_PNG);
    }

    #[test]
    #[ignore = "reads the real pasteboard, run by hand with --ignored"]
    fn system_snapshot_matches_the_change_count() {
        let _lock = lock_system_pasteboard();
        let pasteboard = SystemPasteboard;
        let snapshot = pasteboard.snapshot().expect("pasteboard changed mid-read");
        assert_eq!(snapshot.change_count, pasteboard.change_count());
    }

    #[test]
    #[ignore = "overwrites the real pasteboard, run by hand with --ignored"]
    fn system_write_is_visible_in_the_next_snapshot() {
        let _lock = lock_system_pasteboard();
        let pasteboard = SystemPasteboard;
        let before = pasteboard.change_count();
        assert!(pasteboard.write_text("packrat pasteboard test"));

        let snapshot = pasteboard.snapshot().expect("pasteboard changed mid-read");
        assert!(snapshot.change_count > before);
        assert_eq!(snapshot.text.as_deref(), Some("packrat pasteboard test"));
        assert!(!snapshot.concealed);
    }

    #[test]
    #[ignore = "overwrites the real pasteboard, run by hand with --ignored"]
    fn system_image_write_is_visible_in_the_next_snapshot() {
        let _lock = lock_system_pasteboard();
        let pasteboard = SystemPasteboard;
        assert!(pasteboard.write_image(TINY_PNG));

        let snapshot = pasteboard.snapshot().expect("pasteboard changed mid-read");
        assert_eq!(snapshot.text, None);
        let image = snapshot.image.expect("no image on the pasteboard");
        assert_eq!(image.png, TINY_PNG);
    }
}
