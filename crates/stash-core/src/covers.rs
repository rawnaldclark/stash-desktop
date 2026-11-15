//! The picture of a file of the library: the one inside the file, else the one beside it in its folder.
//!
//! A row's `art` is an address. For a song another Stash described it is the picture's address on the web. For a
//! file here it is the core's own address for the file's picture ([`address`]), which the local API answers by
//! reading the file ([`of_file`]). Whether a file has a picture is found out the first time it is asked for and
//! kept (`tracks.cover`), so a file without one is asked for once.

use std::fs;
use std::path::Path;

use lofty::file::TaggedFileExt;
use lofty::picture::PictureType;

/// The start of the core's own address for a file's picture; the track's id follows.
pub const ADDRESS: &str = "/api/art/track/";

/// The core's own address for the picture of the file that is track `id`.
pub fn address(id: i64) -> String {
    format!("{ADDRESS}{id}")
}

/// A picture, and what kind of image its bytes are.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cover {
    pub content_type: &'static str,
    pub bytes: Vec<u8>,
}

/// The names a folder's picture goes by, without the extension, in the order they are looked for.
const BESIDE: [&str; 4] = ["cover", "folder", "front", "album"];
const EXTENSIONS: [&str; 4] = ["jpg", "jpeg", "png", "webp"];
/// A picture beside the files larger than this is not read.
const LARGEST: u64 = 32 * 1024 * 1024;

/// What kind of image these bytes are, from the bytes themselves. What a tag says its picture is is not trusted:
/// only images a window draws are handed out, under their true name.
fn kind(bytes: &[u8]) -> Option<&'static str> {
    match bytes {
        [0xFF, 0xD8, 0xFF, ..] => Some("image/jpeg"),
        [0x89, b'P', b'N', b'G', ..] => Some("image/png"),
        [b'G', b'I', b'F', b'8', ..] => Some("image/gif"),
        [
            b'R',
            b'I',
            b'F',
            b'F',
            _,
            _,
            _,
            _,
            b'W',
            b'E',
            b'B',
            b'P',
            ..,
        ] => Some("image/webp"),
        [b'B', b'M', ..] => Some("image/bmp"),
        _ => None,
    }
}

/// The picture of the music file at `path`: the front cover inside it, else any picture inside it, else the
/// folder's picture. `None` when there is none (or the file cannot be read).
pub fn of_file(path: &Path) -> Option<Cover> {
    inside(path).or_else(|| beside(path))
}

fn inside(path: &Path) -> Option<Cover> {
    let file = lofty::read_from_path(path).ok()?;
    let pictures = || file.tags().iter().flat_map(|tag| tag.pictures());
    let usable = |picture: &&lofty::picture::Picture| kind(picture.data()).is_some();
    let picture = pictures()
        .filter(usable)
        .find(|picture| picture.pic_type() == PictureType::CoverFront)
        .or_else(|| pictures().find(usable))?;
    Some(Cover {
        content_type: kind(picture.data())?,
        bytes: picture.data().to_vec(),
    })
}

fn beside(path: &Path) -> Option<Cover> {
    let folder = path.parent()?;
    let mut found: Vec<(usize, std::path::PathBuf)> = fs::read_dir(folder)
        .ok()?
        .filter_map(|entry| entry.ok())
        .filter_map(|entry| {
            let path = entry.path();
            let stem = path.file_stem()?.to_str()?.to_ascii_lowercase();
            let extension = path.extension()?.to_str()?.to_ascii_lowercase();
            let rank = BESIDE.iter().position(|name| *name == stem)?;
            EXTENSIONS
                .contains(&extension.as_str())
                .then_some((rank, path))
        })
        .collect();
    found.sort();
    found.into_iter().find_map(|(_, path)| {
        let size = fs::metadata(&path).ok()?.len();
        if size == 0 || size > LARGEST {
            return None;
        }
        let bytes = fs::read(&path).ok()?;
        Some(Cover {
            content_type: kind(&bytes)?,
            bytes,
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_image_is_known_by_its_bytes_and_nothing_else_is_an_image() {
        assert_eq!(kind(&[0xFF, 0xD8, 0xFF, 0xE0, 0, 0]), Some("image/jpeg"));
        assert_eq!(kind(b"\x89PNG\r\n\x1a\n"), Some("image/png"));
        assert_eq!(kind(b"RIFF\x10\0\0\0WEBPVP8 "), Some("image/webp"));
        assert_eq!(kind(b"<svg xmlns='http://www.w3.org/2000/svg'>"), None);
        assert_eq!(kind(b"<!doctype html>"), None);
        assert_eq!(kind(b""), None);
    }

    #[test]
    fn the_folders_picture_is_found_by_its_name_whatever_its_case() {
        let dir = std::env::temp_dir().join(format!("stash-covers-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let song = dir.join("song.flac");
        fs::write(&song, b"not really music").unwrap();
        assert_eq!(of_file(&song), None);

        // A file with a picture's name that is not a picture is not handed out.
        fs::write(dir.join("cover.jpg"), b"<html>").unwrap();
        assert_eq!(of_file(&song), None);
        fs::write(dir.join("Folder.PNG"), b"\x89PNG\r\n\x1a\nrest").unwrap();
        let cover = of_file(&song).unwrap();
        assert_eq!(cover.content_type, "image/png");

        // "cover" is looked for before "folder".
        fs::write(dir.join("cover.jpg"), [0xFF, 0xD8, 0xFF, 0xE0, 1, 2]).unwrap();
        assert_eq!(of_file(&song).unwrap().content_type, "image/jpeg");
        let _ = fs::remove_dir_all(&dir);
    }
}
