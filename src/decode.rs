//! Decoding a photo file into an RGBA thumbnail, done off the main thread.

use std::io::Cursor;
use std::path::Path;

use image::{RgbaImage, imageops};
use turbojpeg::{PixelFormat, ScalingFactor};

/// A decoded, oriented image, small enough to fit in a `target` square.
pub struct Decoded {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

pub fn decode(path: &Path, target: u32) -> Result<Decoded, String> {
    let started = std::time::Instant::now();
    let result = decode_inner(path, target);
    if std::env::var_os("LANTERN_DEBUG").is_some() {
        eprintln!("decode {:>4} ms  @{target:<4} {}", started.elapsed().as_millis(), path.display());
    }
    result
}

fn decode_inner(path: &Path, target: u32) -> Result<Decoded, String> {
    let data = std::fs::read(path).map_err(|e| e.to_string())?;
    let orientation = exif_orientation(&data);

    let image = if data.starts_with(&[0xFF, 0xD8]) {
        decode_jpeg(&data, target)?
    } else {
        decode_other(&data, target)?
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
