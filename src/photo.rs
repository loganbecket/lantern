//! What the grid knows about one photo, and how the date taken is found.

use std::io::Cursor;
use std::path::Path;

use gtk::{gio, glib};

/// EXIF lives near the start of a file; this is plenty for JPEG and for the
/// metadata box of iPhone HEIC files.
const HEADER_BYTES: usize = 64 * 1024;

/// One tile in the grid: a subfolder or a photo.
#[derive(Clone, Debug)]
pub enum Entry {
    Folder { file: gio::File, name: String },
    Photo(PhotoInfo),
}

impl Entry {
    pub fn file(&self) -> &gio::File {
        match self {
            Entry::Folder { file, .. } => file,
            Entry::Photo(photo) => &photo.file,
        }
    }

    pub fn name(&self) -> &str {
        match self {
            Entry::Folder { name, .. } => name,
            Entry::Photo(photo) => &photo.name,
        }
    }

    pub fn photo(&self) -> Option<&PhotoInfo> {
        match self {
            Entry::Photo(photo) => Some(photo),
            Entry::Folder { .. } => None,
        }
    }
}

#[derive(Clone, Debug)]
pub struct PhotoInfo {
    pub file: gio::File,
    pub name: String,
    /// File modification time, unix seconds.
    pub modified: i64,
    /// Date taken from EXIF, unix seconds, once it has been read.
    pub taken: Option<i64>,
}

impl PhotoInfo {
    /// The best available "when": date taken, else modified.
    pub fn when(&self) -> i64 {
        self.taken.unwrap_or(self.modified)
    }
}

/// Read the EXIF date taken, if the file has one.
pub fn read_taken(path: &Path) -> Option<i64> {
    let data = crate::decode::read_prefix(path, HEADER_BYTES)?;
    let exif = exif::Reader::new().read_from_container(&mut Cursor::new(data)).ok()?;

    [exif::Tag::DateTimeOriginal, exif::Tag::DateTime]
        .into_iter()
        .find_map(|tag| parse_exif_datetime(exif.get_field(tag, exif::In::PRIMARY)?))
}

/// EXIF dates look like `2019:07:27 14:03:11` in the camera's local time.
fn parse_exif_datetime(field: &exif::Field) -> Option<i64> {
    let exif::Value::Ascii(values) = &field.value else { return None };
    let text = std::str::from_utf8(values.first()?).ok()?.trim();
    let (date, time) = text.split_once(' ')?;
    let mut ymd = date.split(':').map(|p| p.parse::<i32>().ok());
    let mut hms = time.split(':').map(|p| p.parse::<i32>().ok());
    let (y, m, d) = (ymd.next()??, ymd.next()??, ymd.next()??);
    let (h, mi, s) = (hms.next()??, hms.next()??, hms.next()??);
    glib::DateTime::from_local(y, m, d, h, mi, s as f64).ok().map(|dt| dt.to_unix())
}
