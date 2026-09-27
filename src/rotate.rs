//! Rotating a photo on disk without losing quality.
//!
//! JPEG: if the file has an EXIF orientation tag, only that tag is changed
//! (two bytes; the pixels are untouched and every viewer honors it).
//! Without one, libjpeg-turbo rotates the compressed data losslessly.
//! PNG is lossless anyway, so it is decoded, rotated and re-encoded.
//! Other formats can't be rotated without re-encoding and are refused.
//! The file's modified time is preserved so date order doesn't change.

use std::io::{Seek, SeekFrom, Write};
use std::path::Path;

use crate::decode::PREVIEW_BYTES;

/// EXIF orientation after one more quarter turn on screen, clockwise and
/// counter-clockwise, for each of the eight orientations.
const CLOCKWISE: [u8; 9] = [0, 6, 7, 8, 5, 2, 3, 4, 1];
const COUNTER_CLOCKWISE: [u8; 9] = [0, 8, 5, 6, 7, 4, 1, 2, 3];

pub fn rotate(path: &Path, clockwise: bool) -> Result<(), String> {
    let modified = std::fs::metadata(path).and_then(|m| m.modified()).ok();
    let table = if clockwise { &CLOCKWISE } else { &COUNTER_CLOCKWISE };

    // The common case first: a JPEG with an orientation tag near the start.
    // Two bytes change, so they are written straight into the file; no
    // full read, no full write. Over Wi-Fi that is the difference between
    // instant and a second or more.
    let prefix = crate::decode::read_prefix(path, PREVIEW_BYTES).ok_or("can't read the file")?;
    if prefix.starts_with(&[0xFF, 0xD8]) {
        if let Some((at, big_endian, current)) = exif_orientation_slot(&prefix) {
            let next = table[current as usize];
            let bytes = if big_endian { (next as u16).to_be_bytes() } else { (next as u16).to_le_bytes() };
            let mut file = std::fs::File::options().write(true).open(path).map_err(|e| e.to_string())?;
            file.seek(SeekFrom::Start(at as u64)).and_then(|_| file.write_all(&bytes)).map_err(|e| e.to_string())?;
            if let Some(modified) = modified {
                let _ = file.set_modified(modified);
            }
            return Ok(());
        }
    }

    let mut data = std::fs::read(path).map_err(|e| e.to_string())?;
    let output = if data.starts_with(&[0xFF, 0xD8]) {
        match exif_orientation_slot(&data) {
            Some((at, big_endian, current)) => {
                let next = table[current as usize];
                let bytes = if big_endian { (next as u16).to_be_bytes() } else { (next as u16).to_le_bytes() };
                data[at..at + 2].copy_from_slice(&bytes);
                data
            }
            None => {
                let op = if clockwise { turbojpeg::TransformOp::Rot90 } else { turbojpeg::TransformOp::Rot270 };
                let mut transform = turbojpeg::Transform::op(op);
                transform.trim = true;
                turbojpeg::transform(&transform, &data).map_err(|e| e.to_string())?.to_vec()
            }
        }
    } else if data.starts_with(b"\x89PNG") {
        let image = image::load_from_memory(&data).map_err(|e| e.to_string())?;
        let rotated = if clockwise { image.rotate90() } else { image.rotate270() };
        let mut out = std::io::Cursor::new(Vec::new());
        rotated.write_to(&mut out, image::ImageFormat::Png).map_err(|e| e.to_string())?;
        out.into_inner()
    } else {
        return Err("Only JPEG and PNG can be rotated without losing quality".into());
    };

    write_in_place(path, &output)?;
    if let Some(modified) = modified {
        let _ = std::fs::File::options().write(true).open(path).and_then(|f| f.set_modified(modified));
    }
    Ok(())
}

/// Write to a temporary file beside the original, then rename over it, so
/// a crash or full disk can't leave a half-written photo.
fn write_in_place(path: &Path, data: &[u8]) -> Result<(), String> {
    let temp = path.with_extension(format!("{}.lantern-tmp", path.extension().and_then(|e| e.to_str()).unwrap_or("")));
    std::fs::write(&temp, data).and_then(|_| std::fs::rename(&temp, path)).map_err(|e| {
        let _ = std::fs::remove_file(&temp);
        e.to_string()
    })
}

/// Where a JPEG's EXIF orientation value lives: byte offset of the two-byte
/// value, whether the TIFF data is big-endian, and the current value.
fn exif_orientation_slot(data: &[u8]) -> Option<(usize, bool, u8)> {
    let mut pos = 2;
    while pos + 4 <= data.len() && data[pos] == 0xFF {
        let marker = data[pos + 1];
        if marker == 0xDA {
            break; // Start of scan: no more headers.
        }
        let length = u16::from_be_bytes([data[pos + 2], data[pos + 3]]) as usize;
        if marker == 0xE1 && data.get(pos + 4..pos + 10) == Some(b"Exif\0\0") {
            return orientation_in_tiff(data, pos + 10, pos + 2 + length);
        }
        pos += 2 + length;
    }
    None
}

fn orientation_in_tiff(data: &[u8], tiff: usize, end: usize) -> Option<(usize, bool, u8)> {
    let big_endian = match data.get(tiff..tiff + 2)? {
        b"MM" => true,
        b"II" => false,
        _ => return None,
    };
    let u16_at = |at: usize| -> Option<u16> {
        let b = data.get(at..at + 2)?;
        Some(if big_endian { u16::from_be_bytes([b[0], b[1]]) } else { u16::from_le_bytes([b[0], b[1]]) })
    };
    let u32_at = |at: usize| -> Option<u32> {
        let b = data.get(at..at + 4)?;
        Some(if big_endian { u32::from_be_bytes([b[0], b[1], b[2], b[3]]) } else { u32::from_le_bytes([b[0], b[1], b[2], b[3]]) })
    };
    let ifd = tiff + u32_at(tiff + 4)? as usize;
    let entries = u16_at(ifd)? as usize;
    for i in 0..entries {
        let entry = ifd + 2 + i * 12;
        if entry + 12 > end {
            return None;
        }
        if u16_at(entry)? == 0x0112 && u16_at(entry + 2)? == 3 && u32_at(entry + 4)? == 1 {
            let value_at = entry + 8;
            let value = u16_at(value_at)?;
            return (1..=8).contains(&value).then_some((value_at, big_endian, value as u8));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{exif_jpeg, plain_jpeg, scratch, write};
    use std::path::PathBuf;

    fn red_at(path: &Path, x: u32, y: u32) -> u8 {
        image::open(path).unwrap().to_rgb8().get_pixel(x, y)[0]
    }

    #[test]
    fn jpeg_without_exif_rotates_losslessly_and_keeps_mtime() {
        let path = scratch("plain.jpg");
        write(&path, &plain_jpeg(64, 32));
        let before = std::fs::metadata(&path).unwrap().modified().unwrap();
        std::thread::sleep(std::time::Duration::from_millis(20));

        rotate(&path, true).unwrap();
        let rotated = image::open(&path).unwrap();
        assert_eq!((rotated.width(), rotated.height()), (32, 64));
        // The gradient ran left to right; after a clockwise turn it runs
        // top to bottom, so the top is dark and the bottom is bright.
        assert!(red_at(&path, 16, 2) < 60, "top should be dark");
        assert!(red_at(&path, 16, 61) > 180, "bottom should be bright");
        assert_eq!(std::fs::metadata(&path).unwrap().modified().unwrap(), before);
        assert!(exif_orientation_slot(&std::fs::read(&path).unwrap()).is_none());

        rotate(&path, false).unwrap();
        assert!(red_at(&path, 2, 16) < 60, "back to dark on the left");
    }

    #[test]
    fn jpeg_with_exif_changes_only_the_tag() {
        for big_endian in [false, true] {
            let path = scratch(&format!("exif-{big_endian}.jpg"));
            let before = exif_jpeg(64, 32, 1, big_endian, None);
            write(&path, &before);
            let (at, _, value) = exif_orientation_slot(&before).unwrap();
            assert_eq!(value, 1);

            rotate(&path, true).unwrap();
            let after = std::fs::read(&path).unwrap();
            assert_eq!(before.len(), after.len());
            let differing: Vec<usize> = (0..before.len()).filter(|&i| before[i] != after[i]).collect();
            assert!(differing.iter().all(|i| (at..at + 2).contains(i)), "changed bytes {differing:?}");
            assert_eq!(exif_orientation_slot(&after).unwrap().2, 6, "upright turned clockwise is 6");

            rotate(&path, true).unwrap();
            assert_eq!(exif_orientation_slot(&std::fs::read(&path).unwrap()).unwrap().2, 3);

            rotate(&path, false).unwrap();
            rotate(&path, false).unwrap();
            assert_eq!(std::fs::read(&path).unwrap(), before, "two back should restore the file");

            rotate(&path, false).unwrap();
            assert_eq!(exif_orientation_slot(&std::fs::read(&path).unwrap()).unwrap().2, 8);
        }
    }

    /// Measurement, not a test: `LANTERN_TEST_JPEG=/some/photo.jpg cargo
    /// test rotate_timing -- --ignored --nocapture` times ten quarter turns
    /// of a copy of that file (the copy sits beside it, so a file on a
    /// network share measures the share).
    #[test]
    #[ignore = "measurement harness; needs LANTERN_TEST_JPEG"]
    fn rotate_timing() {
        let Some(source) = std::env::var_os("LANTERN_TEST_JPEG") else { return };
        let source = PathBuf::from(source);
        let copy = source.with_extension("lantern-timing.jpg");
        std::fs::copy(&source, &copy).unwrap();
        let started = std::time::Instant::now();
        for i in 0..10 {
            rotate(&copy, i % 2 == 0).unwrap();
        }
        eprintln!("rotate: {} ms per turn, {} bytes", started.elapsed().as_millis() / 10, std::fs::metadata(&copy).unwrap().len());
        std::fs::remove_file(&copy).unwrap();
    }

    #[test]
    fn other_formats_are_left_untouched() {
        let path = scratch("photo.gif");
        image::RgbImage::from_pixel(8, 8, image::Rgb([200, 40, 40])).save(&path).unwrap();
        let before = std::fs::read(&path).unwrap();
        assert!(rotate(&path, true).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), before);
    }

    #[test]
    fn a_failed_write_leaves_no_temp_file() {
        let dir = scratch("missing-dir");
        let path = dir.join("nowhere.jpg");
        assert!(write_in_place(&path, b"x").is_err());
        assert!(std::fs::read_dir(scratch("")).unwrap().flatten().all(|e| !e.file_name().to_string_lossy().contains("lantern-tmp")));
    }
}
