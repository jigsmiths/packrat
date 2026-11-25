use std::path::{Path, PathBuf};

use objc2::rc::autoreleasepool;
use objc2::runtime::AnyObject;
use objc2_app_kit::{
    NSBitmapImageFileType, NSBitmapImageRep, NSPasteboard, NSPasteboardTypeString,
};
use objc2_foundation::{NSData, NSDictionary, NSString, NSURL};

pub const MAX_IMAGE_BYTES: usize = 25 * 1024 * 1024;

// Types that password managers attach to secrets, see nspasteboard.org.
const CONCEALED_TYPES: [&str; 2] = [
    "org.nspasteboard.ConcealedType",
    "org.nspasteboard.TransientType",
];

// The values of AppKit's NSPasteboardType constants, which are unsafe to read.
const FILE_URL_TYPE: &str = "public.file-url";
const PNG_TYPE: &str = "public.png";
const TIFF_TYPE: &str = "public.tiff";

const IMAGE_EXTENSIONS: [&str; 10] = [
    "png", "jpg", "jpeg", "heic", "heif", "gif", "tif", "tiff", "bmp", "webp",
];

const PNG_SIGNATURE: &[u8] = b"\x89PNG\r\n\x1a\n";

#[derive(Clone, PartialEq, Eq)]
pub struct Image {
    pub png: Vec<u8>,
    pub width: usize,
    pub height: usize,
    pub file_name: Option<String>,
}

impl std::fmt::Debug for Image {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Image")
            .field("bytes", &self.png.len())
            .field("width", &self.width)
            .field("height", &self.height)
            .field("file_name", &self.file_name)
            .finish()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Snapshot {
    pub change_count: isize,
    pub text: Option<String>,
    pub image: Option<Image>,
    pub concealed: bool,
}

pub trait Pasteboard {
    fn change_count(&self) -> isize;

    fn snapshot(&self) -> Option<Snapshot>;
}

pub struct SystemPasteboard;

impl Pasteboard for SystemPasteboard {
    fn change_count(&self) -> isize {
        NSPasteboard::generalPasteboard().changeCount()
    }

    fn snapshot(&self) -> Option<Snapshot> {
        autoreleasepool(|_| {
            let pasteboard = NSPasteboard::generalPasteboard();
            let change_count = pasteboard.changeCount();
            let types: Vec<String> = pasteboard
                .types()
                .map(|types| types.iter().map(|name| name.to_string()).collect())
                .unwrap_or_default();
            // SAFETY: NSPasteboardTypeString is an immutable constant exported by AppKit.
            #[allow(unsafe_code)]
            let text = pasteboard
                .stringForType(unsafe { NSPasteboardTypeString })
                .map(|text| text.to_string());
            let concealed = is_concealed(&types);
            let has_file = types.iter().any(|name| name == FILE_URL_TYPE);
            let image = if concealed {
                None
            } else if has_file {
                // Image data next to a file is only its Finder icon.
                copied_file(&pasteboard).and_then(|path| read_image_file(&path))
            } else if text.is_none() {
                image_data(&pasteboard)
            } else {
                None
            };

            (pasteboard.changeCount() == change_count).then_some(Snapshot {
                change_count,
                text,
                image,
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

fn read_image_file(path: &Path) -> Option<Image> {
    if !is_image_file(path) {
        return None;
    }
    let size = std::fs::metadata(path).ok()?.len();
    if size > MAX_IMAGE_BYTES as u64 {
        return None;
    }
    let mut image = to_png(std::fs::read(path).ok()?)?;
    image.file_name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned());
    Some(image)
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
            file_name: None,
        });
    }
    let rep = NSBitmapImageRep::imageRepWithData(&NSData::with_bytes(&bytes))?;
    let properties = NSDictionary::<NSString, AnyObject>::new();
    // SAFETY: an empty dictionary has no values of the wrong type.
    #[allow(unsafe_code)]
    let png =
        unsafe { rep.representationUsingType_properties(NSBitmapImageFileType::PNG, &properties) }?;
    Some(Image {
        png: png.to_vec(),
        width: usize::try_from(rep.pixelsWide()).ok()?,
        height: usize::try_from(rep.pixelsHigh()).ok()?,
        file_name: None,
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

fn is_image_file(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| IMAGE_EXTENSIONS.contains(&ext.to_ascii_lowercase().as_str()))
}

impl SystemPasteboard {
    #[allow(unsafe_code)]
    pub fn write_text(&self, text: &str) -> bool {
        autoreleasepool(|_| {
            let pasteboard = NSPasteboard::generalPasteboard();
            pasteboard.clearContents();
            // SAFETY: as in `snapshot`.
            pasteboard
                .setString_forType(&NSString::from_str(text), unsafe { NSPasteboardTypeString })
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
}

fn is_concealed(types: &[String]) -> bool {
    types
        .iter()
        .any(|name| CONCEALED_TYPES.contains(&name.as_str()))
}

#[cfg(test)]
mod tests {
    use super::*;

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
        assert_eq!(image.file_name, None);
    }

    #[test]
    fn other_formats_are_converted_to_png() {
        let image = to_png(tiny_bmp()).unwrap();
        assert_eq!(png_size(&image.png), Some((2, 1)));
        assert_eq!((image.width, image.height), (2, 1));
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
            assert!(is_image_file(Path::new(name)), "{name}");
        }
        for name in ["notes.txt", "archive.png.zip", "png", "noext"] {
            assert!(!is_image_file(Path::new(name)), "{name}");
        }
    }

    #[test]
    fn copied_image_files_keep_their_name() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("Screenshot.png");
        std::fs::write(&path, TINY_PNG).unwrap();

        let image = read_image_file(&path).unwrap();
        assert_eq!(image.png, TINY_PNG);
        assert_eq!(image.file_name.as_deref(), Some("Screenshot.png"));
    }

    #[test]
    fn unusable_files_are_not_read() {
        let dir = tempfile::TempDir::new().unwrap();
        let text = dir.path().join("notes.txt");
        std::fs::write(&text, "hello").unwrap();
        assert!(read_image_file(&text).is_none());

        assert!(read_image_file(&dir.path().join("missing.png")).is_none());

        let huge = dir.path().join("huge.png");
        let file = std::fs::File::create(&huge).unwrap();
        file.set_len(MAX_IMAGE_BYTES as u64 + 1).unwrap();
        assert!(read_image_file(&huge).is_none());
    }

    #[test]
    #[ignore = "reads the real pasteboard, run by hand with --ignored"]
    fn system_snapshot_matches_the_change_count() {
        let pasteboard = SystemPasteboard;
        let snapshot = pasteboard.snapshot().expect("pasteboard changed mid-read");
        assert_eq!(snapshot.change_count, pasteboard.change_count());
    }

    #[test]
    #[ignore = "overwrites the real pasteboard, run by hand with --ignored"]
    fn system_write_is_visible_in_the_next_snapshot() {
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
        let pasteboard = SystemPasteboard;
        assert!(pasteboard.write_image(TINY_PNG));

        let snapshot = pasteboard.snapshot().expect("pasteboard changed mid-read");
        assert_eq!(snapshot.text, None);
        let image = snapshot.image.expect("no image on the pasteboard");
        assert_eq!(image.png, TINY_PNG);
        assert_eq!(image.file_name, None);
    }
}
