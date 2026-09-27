//! Test fixtures: JPEGs with hand-built EXIF, so the code that reads and
//! edits real camera files can be tested without shipping photos.

use std::path::{Path, PathBuf};

/// A folder for this test process's files.
pub fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("lantern-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir.join(name)
}

/// A plain baseline JPEG, `width` x `height`, red rising left to right.
pub fn plain_jpeg(width: u32, height: u32) -> Vec<u8> {
    let img = image::RgbImage::from_fn(width, height, |x, _| image::Rgb([(x * 255 / width.max(1)) as u8, 40, 40]));
    let mut out = std::io::Cursor::new(Vec::new());
    img.write_to(&mut out, image::ImageFormat::Jpeg).unwrap();
    out.into_inner()
}

/// `plain_jpeg` with an APP1 EXIF segment carrying an orientation tag and,
/// optionally, an embedded thumbnail (a small JPEG) in IFD1, in either
/// byte order. This mirrors what cameras and phones write.
pub fn exif_jpeg(width: u32, height: u32, orientation: u16, big_endian: bool, thumbnail: Option<&[u8]>) -> Vec<u8> {
    let base = plain_jpeg(width, height);
    let tiff = tiff_with_orientation(orientation, big_endian, thumbnail);
    let mut app1 = Vec::new();
    app1.extend_from_slice(b"Exif\0\0");
    app1.extend_from_slice(&tiff);
    let length = (app1.len() + 2) as u16;

    let mut out = Vec::with_capacity(base.len() + app1.len() + 4);
    out.extend_from_slice(&base[..2]);
    out.extend_from_slice(&[0xFF, 0xE1]);
    out.extend_from_slice(&length.to_be_bytes());
    out.extend_from_slice(&app1);
    out.extend_from_slice(&base[2..]);
    out
}

fn tiff_with_orientation(orientation: u16, big_endian: bool, thumbnail: Option<&[u8]>) -> Vec<u8> {
    let u16b = |v: u16| if big_endian { v.to_be_bytes() } else { v.to_le_bytes() };
    let u32b = |v: u32| if big_endian { v.to_be_bytes() } else { v.to_le_bytes() };
    let entry = |out: &mut Vec<u8>, tag: u16, kind: u16, count: u32, value: [u8; 4]| {
        out.extend_from_slice(&u16b(tag));
        out.extend_from_slice(&u16b(kind));
        out.extend_from_slice(&u32b(count));
        out.extend_from_slice(&value);
    };
    let short = |v: u16| {
        let mut b = [0u8; 4];
        b[..2].copy_from_slice(&u16b(v));
        b
    };

    let mut tiff = Vec::new();
    tiff.extend_from_slice(if big_endian { b"MM" } else { b"II" });
    tiff.extend_from_slice(&u16b(42));
    tiff.extend_from_slice(&u32b(8));

    // IFD0: orientation only. 2 + 12 + 4 = 18 bytes, so IFD1 starts at 26.
    let ifd1_offset: u32 = if thumbnail.is_some() { 26 } else { 0 };
    tiff.extend_from_slice(&u16b(1));
    entry(&mut tiff, 0x0112, 3, 1, short(orientation));
    tiff.extend_from_slice(&u32b(ifd1_offset));

    if let Some(thumb) = thumbnail {
        // IFD1: JPEGInterchangeFormat + length, data right after the IFD.
        let data_offset = 26 + 2 + 2 * 12 + 4;
        tiff.extend_from_slice(&u16b(2));
        entry(&mut tiff, 0x0201, 4, 1, u32b(data_offset));
        entry(&mut tiff, 0x0202, 4, 1, u32b(thumb.len() as u32));
        tiff.extend_from_slice(&u32b(0));
        tiff.extend_from_slice(thumb);
    }
    tiff
}

pub fn write(path: &Path, data: &[u8]) {
    std::fs::write(path, data).unwrap();
}
