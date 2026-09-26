//! Decoding a photo file into an RGBA thumbnail, done off the main thread.

use std::io::{Cursor, Read};
use std::os::fd::AsRawFd;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

use image::{RgbaImage, imageops};
use turbojpeg::{PixelFormat, ScalingFactor};

/// Read a whole file in large chunks, giving up as soon as `cancel` is set
/// so a cell that scrolled away stops eating bandwidth.
fn read_all(path: &Path, cancel: &AtomicBool) -> std::io::Result<Vec<u8>> {
    const CHUNK: usize = 512 * 1024;
    let mut file = std::fs::File::open(path)?;
    let len = file.metadata().map(|m| m.len() as usize).unwrap_or(0);
    let mut data = Vec::with_capacity(len);
    let mut buf = vec![0u8; CHUNK];
    loop {
        if cancel.load(Ordering::Relaxed) {
            return Err(std::io::Error::other("canceled"));
        }
        match file.read(&mut buf)? {
            0 => return Ok(data),
            n => data.extend_from_slice(&buf[..n]),
        }
    }
}

/// A decoded, oriented image, small enough to fit in a `target` square.
pub struct Decoded {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

pub fn decode(path: &Path, target: u32, cancel: &AtomicBool) -> Result<Decoded, String> {
    let started = std::time::Instant::now();
    let result = decode_inner(path, target, cancel);
    if std::env::var_os("LANTERN_DEBUG").is_some() && !cancel.load(Ordering::Relaxed) {
        eprintln!("decode {:>4} ms  @{target:<4} {}", started.elapsed().as_millis(), path.display());
    }
    result
}

fn decode_inner(path: &Path, target: u32, cancel: &AtomicBool) -> Result<Decoded, String> {
    let data = read_all(path, cancel).map_err(|e| e.to_string())?;

    // libheif applies the file's own rotation, so HEIF skips the EXIF step.
    let (image, orientation) = if data.starts_with(&[0xFF, 0xD8]) {
        // libjpeg-turbo rejects some slightly malformed JPEGs that the
        // tolerant pure-Rust decoder still opens.
        let image = decode_jpeg(&data, target).or_else(|_| decode_other(&data, target))?;
        (image, exif_orientation(&data))
    } else if is_heif(&data) {
        (decode_heif(&data, target)?, 1)
    } else {
        (decode_other(&data, target)?, exif_orientation(&data))
    };

    let image = fit(image, target);
    let image = orient(image, orientation);
    Ok(Decoded { width: image.width(), height: image.height(), rgba: image.into_raw() })
}

/// JPEGs can be decoded directly at 1/2, 1/4 or 1/8 size, which is where most
/// of the speed comes from: an 8x smaller decode is roughly 8x faster.
fn decode_jpeg(data: &[u8], target: u32) -> Result<RgbaImage, String> {
    let mut decompressor = turbojpeg::Decompressor::new().map_err(|e| e.to_string())?;
    let header = decompressor.read_header(data).map_err(|e| e.to_string())?;

    let longest = header.width.max(header.height);
    let factor = [ScalingFactor::ONE_EIGHTH, ScalingFactor::ONE_QUARTER, ScalingFactor::ONE_HALF]
        .into_iter()
        .find(|f| f.scale(longest) >= target as usize)
        .unwrap_or(ScalingFactor::ONE);
    decompressor.set_scaling_factor(factor).map_err(|e| e.to_string())?;

    let scaled = header.scaled(factor);
    let mut pixels = vec![0u8; scaled.width * scaled.height * 4];
    let output = turbojpeg::Image {
        pixels: pixels.as_mut_slice(),
        width: scaled.width,
        pitch: scaled.width * 4,
        height: scaled.height,
        format: PixelFormat::RGBA,
    };
    decompressor.decompress(data, output).map_err(|e| e.to_string())?;

    RgbaImage::from_raw(scaled.width as u32, scaled.height as u32, pixels)
        .ok_or_else(|| "JPEG buffer size mismatch".to_string())
}

/// How much of a file the preview path reads. Cameras put the EXIF
/// thumbnail in the first few tens of KB (64 KB caught every one in a
/// 189-file sample), and over Wi-Fi bytes are what cost.
pub const PREVIEW_BYTES: usize = 64 * 1024;

/// A rough, tiny version of the photo from the embedded thumbnail that
/// cameras and phones write into the file header. It needs only the first
/// `PREVIEW_BYTES`, which over Wi-Fi is the difference between a screenful
/// per second and ten per second.
pub fn preview(path: &Path) -> Option<Decoded> {
    let started = std::time::Instant::now();
    let result = preview_inner(path);
    if std::env::var_os("LANTERN_DEBUG").is_some() {
        let outcome = if result.is_some() { "ok" } else { "none" };
        eprintln!("preview {:>4} ms  {outcome:<4} {}", started.elapsed().as_millis(), path.display());
    }
    result
}

fn preview_inner(path: &Path) -> Option<Decoded> {
    // One open, one read: on a network share each round trip costs more
    // than the extra bytes.
    let file = std::fs::File::open(path).ok()?;
    let prefix = read_prefix_of(&file, PREVIEW_BYTES)?;
    // Only JPEG has a cheap preview. iPhone HEIC thumbnails sit at the end
    // of the file and libheif 1.17 reads the whole file to reach them, so
    // they go through the full path, which already prefers the thumbnail.
    let image = jpeg_exif_thumbnail(&prefix)?;
    Some(Decoded { width: image.width(), height: image.height(), rgba: image.into_raw() })
}

/// The first `len` bytes of a file, in as few reads as possible. On a
/// network share every read is a round trip, so growing a buffer 8 KB at a
/// time (as `read_to_end` does) costs a hundred milliseconds per file.
pub fn read_prefix(path: &Path, len: usize) -> Option<Vec<u8>> {
    read_prefix_of(&std::fs::File::open(path).ok()?, len)
}

fn read_prefix_of(mut file: &std::fs::File, len: usize) -> Option<Vec<u8>> {
    // Ask the kernel to fetch the range in one go rather than page by page.
    unsafe {
        libc::posix_fadvise(file.as_raw_fd(), 0, len as libc::off_t, libc::POSIX_FADV_WILLNEED);
    }
    let mut data = vec![0u8; len];
    let mut filled = 0;
    while filled < len {
        match file.read(&mut data[filled..]).ok()? {
            0 => break,
            n => filled += n,
        }
    }
    data.truncate(filled);
    Some(data)
}

fn jpeg_exif_thumbnail(data: &[u8]) -> Option<RgbaImage> {
    if !data.starts_with(&[0xFF, 0xD8]) {
        return None;
    }
    let exif = exif::Reader::new().read_from_container(&mut Cursor::new(data)).ok()?;
    let offset = exif.get_field(exif::Tag::JPEGInterchangeFormat, exif::In::THUMBNAIL)?.value.get_uint(0)? as usize;
    let length = exif.get_field(exif::Tag::JPEGInterchangeFormatLength, exif::In::THUMBNAIL)?.value.get_uint(0)? as usize;
    let thumb = exif.buf().get(offset..offset.checked_add(length)?)?;
    let orientation = exif
        .get_field(exif::Tag::Orientation, exif::In::PRIMARY)
        .and_then(|f| f.value.get_uint(0))
        .unwrap_or(1);
    Some(orient(decode_jpeg(thumb, 1).ok()?, orientation))
}

/// HEIF files start with an `ftyp` box naming a HEIF/AVIF brand.
fn is_heif(data: &[u8]) -> bool {
    const BRANDS: &[&[u8; 4]] = &[b"heic", b"heix", b"hevc", b"hevx", b"heim", b"heis", b"mif1", b"msf1", b"avif"];
    data.len() >= 12 && &data[4..8] == b"ftyp" && BRANDS.contains(&data[8..12].try_into().unwrap())
}

/// HEIC (iPhone) can't be decoded at reduced size like JPEG, so this is the
/// slow path. iPhone files carry a small embedded thumbnail, which is used
/// whenever it is big enough for the target.
fn decode_heif(data: &[u8], target: u32) -> Result<RgbaImage, String> {
    use libheif_rs::{ColorSpace, HeifContext, ItemId, LibHeif, RgbChroma};

    let mut context = HeifContext::read_from_bytes(data).map_err(|e| e.to_string())?;
    // One thread per file; the pool already runs several files at once.
    context.set_max_decoding_threads(1);
    let primary = context.primary_image_handle().map_err(|e| e.to_string())?;

    let lib = LibHeif::new();
    let decode = |handle: &_| lib.decode(handle, ColorSpace::Rgb(RgbChroma::Rgba), None);

    let mut ids = [0 as ItemId; 8];
    let count = primary.thumbnail_ids(&mut ids);
    let thumbnail = ids[..count]
        .iter()
        .filter_map(|id| primary.thumbnail(*id).ok())
        .find(|t| t.width().max(t.height()) >= target)
        // Some thumbnails are JPEG-coded, which older libheif can't decode;
        // fall through to the full image rather than fail.
        .and_then(|t| decode(&t).ok());

    let image = match thumbnail {
        Some(image) => image,
        None => decode(&primary).map_err(|e| e.to_string())?,
    };
    let plane = image.planes().interleaved.ok_or("HEIF decode produced no pixels")?;

    let (w, h) = (plane.width, plane.height);
    let row = w as usize * 4;
    let mut rgba = Vec::with_capacity(row * h as usize);
    for y in 0..h as usize {
        rgba.extend_from_slice(&plane.data[y * plane.stride..y * plane.stride + row]);
    }
    RgbaImage::from_raw(w, h, rgba).ok_or_else(|| "HEIF buffer size mismatch".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `LANTERN_TEST_DIR=/some/photos cargo test previews -- --nocapture`
    /// reports how many files in a folder yield a preview and how big it is.
    #[test]
    fn previews() {
        let Some(dir) = std::env::var_os("LANTERN_TEST_DIR") else { return };
        let mut ok = 0;
        let mut total = 0;
        let mut smallest = u32::MAX;
        let (mut hits_64, mut hits_128) = (0, 0);
        let started = std::time::Instant::now();
        for entry in std::fs::read_dir(dir).unwrap().flatten().take(200) {
            let path = entry.path();
            let ext = path.extension().and_then(|e| e.to_str()).map(|e| e.to_ascii_lowercase());
            if !matches!(ext.as_deref(), Some("jpg" | "jpeg" | "heic")) {
                continue;
            }
            total += 1;
            match preview(&path) {
                Some(p) => {
                    ok += 1;
                    smallest = smallest.min(p.width.max(p.height));
                }
                None => eprintln!("no preview: {}", path.display()),
            }
            // How much of the prefix was really needed?
            if let Some(prefix) = read_prefix(&path, PREVIEW_BYTES) {
                for (kb, hits) in [(64, &mut hits_64), (128, &mut hits_128)] {
                    if jpeg_exif_thumbnail(&prefix[..prefix.len().min(kb * 1024)]).is_some() {
                        *hits += 1;
                    }
                }
            }
        }
        eprintln!("previews: {ok} of {total} in {} ms, smallest longest-edge {smallest}", started.elapsed().as_millis());
        eprintln!("hits with only 64 KB: {hits_64}, 128 KB: {hits_128}");
    }
}

fn decode_other(data: &[u8], _target: u32) -> Result<RgbaImage, String> {
    image::ImageReader::new(Cursor::new(data))
        .with_guessed_format()
        .map_err(|e| e.to_string())?
        .decode()
        .map(|img| img.into_rgba8())
        .map_err(|e| e.to_string())
}

/// Shrink so the longer edge is at most `target`, keeping the aspect ratio.
fn fit(image: RgbaImage, target: u32) -> RgbaImage {
    let (w, h) = image.dimensions();
    let longest = w.max(h);
    if longest <= target {
        return image;
    }
    let scale = target as f64 / longest as f64;
    let nw = ((w as f64 * scale).round() as u32).max(1);
    let nh = ((h as f64 * scale).round() as u32).max(1);
    imageops::thumbnail(&image, nw, nh)
}

fn exif_orientation(data: &[u8]) -> u32 {
    exif::Reader::new()
        .read_from_container(&mut Cursor::new(data))
        .ok()
        .and_then(|e| e.get_field(exif::Tag::Orientation, exif::In::PRIMARY)?.value.get_uint(0))
        .unwrap_or(1)
}

/// Apply an EXIF orientation (1-8) so the image displays upright.
fn orient(image: RgbaImage, orientation: u32) -> RgbaImage {
    match orientation {
        2 => imageops::flip_horizontal(&image),
        3 => imageops::rotate180(&image),
        4 => imageops::flip_vertical(&image),
        5 => imageops::flip_horizontal(&imageops::rotate90(&image)),
        6 => imageops::rotate90(&image),
        7 => imageops::flip_horizontal(&imageops::rotate270(&image)),
        8 => imageops::rotate270(&image),
        _ => image,
    }
}
