//! The station's brand for listeners: its name, and a logo in `logo` messages.
//!
//! A `brand` message gives the name and says how many `logo` parts follow, as the `now`
//! message does for a cover. Pictures from the station (the logo, an item's picture) are
//! read by [`load_picture`], which keeps them small.

use crate::now::PART;
use base64::Engine;
use serde_json::json;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

/// A picture as listeners get it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Picture {
    pub mime: String,
    pub data: Vec<u8>,
}

/// A picture file that could not be used, and why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PictureError {
    pub path: PathBuf,
    pub reason: String,
}

/// Picture errors that nobody has taken yet.
pub(crate) type PictureErrors = Arc<Mutex<Vec<PictureError>>>;

/// The station reads no picture file that is larger than this.
const MAX_FILE: u64 = 4 * 1024 * 1024;
/// The long edge of a picture, in pixels.
const MAX_EDGE: u32 = 512;
/// The JPEG quality of a picture without alpha.
const JPEG_QUALITY: u8 = 85;

/// An encoded picture is at most this large, so that it passes on between listener pages.
const MAX_ENCODED: usize = 120 * 1024;
/// A picture that is too detailed shrinks, but not below this long edge.
const MIN_EDGE: u32 = 64;
/// A source picture has at most this many pixels on an edge.
const MAX_SOURCE_EDGE: u32 = 8192;
/// Decoding allocates at most this much.
const MAX_ALLOC: u64 = 256 * 1024 * 1024;
/// The error list keeps this many entries, the newest.
const MAX_ERRORS: usize = 32;

const NOT_A_PICTURE: &str = "the file is not a PNG, JPEG or WebP picture";

/// Adds an error to the list. When the list is full, the oldest goes.
pub(crate) fn record(errors: &PictureErrors, error: PictureError) {
    if let Ok(mut list) = errors.lock() {
        if list.len() >= MAX_ERRORS {
            list.remove(0);
        }
        list.push(error);
    }
}

/// The brand as listeners get it, with a revision that grows on each change.
#[derive(Default)]
pub(crate) struct Brand {
    pub(crate) rev: u64,
    /// Counts the calls of `set_brand`, so that a slow logo load cannot replace a newer brand.
    pub(crate) generation: u64,
    /// `None` until the station sets a brand.
    pub(crate) name: Option<String>,
    pub(crate) logo: Option<Picture>,
    pub(crate) messages: Arc<Vec<String>>,
}

pub(crate) type SharedBrand = Arc<Mutex<Brand>>;

/// Sets the brand. A logo file is read by a thread, and the brand goes out when it is read.
pub(crate) fn set(
    brand: &SharedBrand,
    errors: &PictureErrors,
    name: String,
    logo: Option<PathBuf>,
) {
    let _ = set_with(brand, errors, name, logo, load_picture);
}

/// [`set`] with the step that loads the logo given. Returns the thread that loads it.
fn set_with(
    brand: &SharedBrand,
    errors: &PictureErrors,
    name: String,
    logo: Option<PathBuf>,
    load: impl FnOnce(&Path) -> Result<Picture, String> + Send + 'static,
) -> Option<std::thread::JoinHandle<()>> {
    let generation = {
        let Ok(mut b) = brand.lock() else { return None };
        b.generation += 1;
        b.name = Some(name.clone());
        b.generation
    };
    let Some(path) = logo else {
        store(brand, generation, name, None);
        return None;
    };
    let brand = brand.clone();
    let errors = errors.clone();
    std::thread::Builder::new()
        .name("webrtc-brand".into())
        .spawn(move || {
            let picture = match load(&path) {
                Ok(p) => Some(p),
                Err(reason) => {
                    // A newer brand replaced this one, and its logo is no one's concern.
                    let current = brand.lock().is_ok_and(|b| b.generation == generation);
                    if current {
                        record(&errors, PictureError { path, reason });
                    }
                    None
                }
            };
            store(&brand, generation, name, picture);
        })
        .ok()
}

fn store(brand: &SharedBrand, generation: u64, name: String, logo: Option<Picture>) {
    let Ok(mut b) = brand.lock() else { return };
    if b.generation != generation {
        return;
    }
    b.rev += 1;
    let id = b.rev;
    b.messages = Arc::new(brand_messages(&name, id, logo.as_ref()));
    b.logo = logo;
}

/// The `GET /brand` body: the name, and the logo as a data URI.
pub(crate) fn brand_json(brand: &SharedBrand) -> Option<String> {
    let b = brand.lock().ok()?;
    let name = b.name.as_ref()?;
    let logo = b.logo.as_ref().map(|l| {
        format!(
            "data:{};base64,{}",
            l.mime,
            base64::engine::general_purpose::STANDARD.encode(&l.data)
        )
    });
    Some(json!({ "name": name, "logo": logo }).to_string())
}

/// Reads a picture file: PNG, JPEG or WebP, at most 4 MB. The picture is scaled down to at
/// most 512 px on the long edge. It stays PNG when it has alpha, and becomes JPEG when not.
pub fn load_picture(path: &Path) -> Result<Picture, String> {
    let bytes = read_file(path)?;
    let format = image::guess_format(&bytes).map_err(|_| NOT_A_PICTURE.to_owned())?;
    if !matches!(
        format,
        image::ImageFormat::Png | image::ImageFormat::Jpeg | image::ImageFormat::WebP
    ) {
        return Err(NOT_A_PICTURE.into());
    }
    decode_and_fit(&bytes, format)
}

/// A picture in the bytes of a cover (PNG, JPEG, WebP or GIF), decoded and encoded again
/// as [`load_picture`] does.
pub(crate) fn refit(bytes: &[u8]) -> Result<Picture, String> {
    let format = image::guess_format(bytes).map_err(|_| NOT_A_PICTURE.to_owned())?;
    if !matches!(
        format,
        image::ImageFormat::Png
            | image::ImageFormat::Jpeg
            | image::ImageFormat::WebP
            | image::ImageFormat::Gif
    ) {
        return Err(NOT_A_PICTURE.into());
    }
    decode_and_fit(bytes, format)
}

fn decode_and_fit(bytes: &[u8], format: image::ImageFormat) -> Result<Picture, String> {
    let mut reader = image::ImageReader::with_format(std::io::Cursor::new(bytes), format);
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(MAX_SOURCE_EDGE);
    limits.max_image_height = Some(MAX_SOURCE_EDGE);
    limits.max_alloc = Some(MAX_ALLOC);
    reader.limits(limits);
    let picture = reader.decode().map_err(|e| match e {
        image::ImageError::Limits(_) => {
            format!("the picture is larger than {MAX_SOURCE_EDGE} pixels")
        }
        _ => NOT_A_PICTURE.to_owned(),
    })?;
    fit(&picture, MAX_ENCODED)
}

/// Encodes the picture in at most `max_bytes`. A picture that is too detailed shrinks until
/// it fits, because a page that passes the logo on sends it in one burst.
fn fit(picture: &image::DynamicImage, max_bytes: usize) -> Result<Picture, String> {
    let mut edge = MAX_EDGE.min(picture.width().max(picture.height()));
    loop {
        let scaled = if edge < picture.width().max(picture.height()) {
            picture.resize(edge, edge, image::imageops::FilterType::Lanczos3)
        } else {
            picture.clone()
        };
        let encoded = encode(&scaled)?;
        if encoded.data.len() <= max_bytes {
            return Ok(encoded);
        }
        edge = edge * 3 / 4;
        if edge < MIN_EDGE {
            return Err("the picture is too detailed to send to listeners".into());
        }
    }
}

fn encode(picture: &image::DynamicImage) -> Result<Picture, String> {
    let failed = |_| "the picture could not be encoded".to_owned();
    let mut data = Vec::new();
    let mime = if picture.color().has_alpha() {
        picture
            .to_rgba8()
            .write_to(
                &mut std::io::Cursor::new(&mut data),
                image::ImageFormat::Png,
            )
            .map_err(failed)?;
        "image/png"
    } else {
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut data, JPEG_QUALITY)
            .encode_image(&picture.to_rgb8())
            .map_err(failed)?;
        "image/jpeg"
    };
    Ok(Picture {
        mime: mime.into(),
        data,
    })
}

fn read_file(path: &Path) -> Result<Vec<u8>, String> {
    let unreadable = |e: std::io::Error| match e.kind() {
        std::io::ErrorKind::NotFound => "the file does not exist".to_owned(),
        _ => "the file cannot be read".to_owned(),
    };
    let file = std::fs::File::open(path).map_err(unreadable)?;
    let meta = file.metadata().map_err(unreadable)?;
    if !meta.is_file() {
        return Err("the file does not exist".into());
    }
    if meta.len() > MAX_FILE {
        return Err("the file is larger than 4 MB".into());
    }
    // The file may grow after the check, so the read has its own limit.
    let mut bytes = Vec::new();
    std::io::Read::read_to_end(&mut std::io::Read::take(file, MAX_FILE + 1), &mut bytes)
        .map_err(unreadable)?;
    if bytes.len() as u64 > MAX_FILE {
        return Err("the file is larger than 4 MB".into());
    }
    Ok(bytes)
}

/// The `brand` message, then the `logo` parts of the picture, if any.
pub fn brand_messages(name: &str, id: u64, logo: Option<&Picture>) -> Vec<String> {
    let head =
        logo.map(|l| json!({ "id": id, "mime": l.mime, "parts": l.data.len().div_ceil(PART) }));
    let mut out = vec![json!({ "type": "brand", "name": name, "logo": head }).to_string()];
    if let Some(l) = logo {
        let b64 = base64::engine::general_purpose::STANDARD;
        for (i, chunk) in l.data.chunks(PART).enumerate() {
            out.push(
                json!({ "type": "logo", "id": id, "part": i, "data": b64.encode(chunk) })
                    .to_string(),
            );
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{DynamicImage, ImageFormat, RgbImage, RgbaImage};
    use std::io::Cursor;

    /// A file in a folder of its own that goes away with the test.
    struct Scratch(PathBuf);

    impl Scratch {
        fn new(test: &str) -> Self {
            let dir =
                std::env::temp_dir().join(format!("radio-stream-{test}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }

        fn file(&self, name: &str, bytes: &[u8]) -> PathBuf {
            let path = self.0.join(name);
            std::fs::write(&path, bytes).unwrap();
            path
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn encode(image: &DynamicImage, format: ImageFormat) -> Vec<u8> {
        let mut out = Cursor::new(Vec::new());
        image.write_to(&mut out, format).unwrap();
        out.into_inner()
    }

    #[test]
    fn a_large_picture_is_scaled_down() {
        let dir = Scratch::new("scaled");
        let big = DynamicImage::ImageRgb8(RgbImage::new(2000, 1000));
        let path = dir.file("big.png", &encode(&big, ImageFormat::Png));
        let picture = load_picture(&path).unwrap();
        let out = image::load_from_memory(&picture.data).unwrap();
        let (w, h) = (out.width(), out.height());
        assert!(w.max(h) <= 512, "{w}x{h}");
        assert!(w.abs_diff(2 * h) <= 1, "aspect kept: {w}x{h}");
    }

    #[test]
    fn a_picture_with_alpha_stays_png_and_one_without_becomes_jpeg() {
        let dir = Scratch::new("alpha");
        let clear =
            DynamicImage::ImageRgba8(RgbaImage::from_pixel(8, 8, image::Rgba([1, 2, 3, 100])));
        let solid = DynamicImage::ImageRgb8(RgbImage::new(8, 8));
        let with = load_picture(&dir.file("a.png", &encode(&clear, ImageFormat::Png))).unwrap();
        let without = load_picture(&dir.file("b.png", &encode(&solid, ImageFormat::Png))).unwrap();
        assert_eq!(with.mime, "image/png");
        assert_eq!(image::guess_format(&with.data).unwrap(), ImageFormat::Png);
        assert_eq!(without.mime, "image/jpeg");
        assert_eq!(
            image::guess_format(&without.data).unwrap(),
            ImageFormat::Jpeg
        );
    }

    #[test]
    fn a_text_file_is_not_a_picture() {
        let dir = Scratch::new("text");
        let path = dir.file("logo.png", b"this is not a picture");
        let reason = load_picture(&path).unwrap_err();
        assert!(!reason.is_empty());
    }

    #[test]
    fn a_missing_file_says_so() {
        let dir = Scratch::new("missing");
        let reason = load_picture(&dir.0.join("none.png")).unwrap_err();
        assert!(!reason.is_empty());
    }

    #[test]
    fn a_file_over_the_limit_is_refused_before_it_is_decoded() {
        let dir = Scratch::new("limit");
        let path = dir.file("huge.png", &vec![0u8; 4 * 1024 * 1024 + 1]);
        let reason = load_picture(&path).unwrap_err();
        assert!(reason.contains("larger"), "{reason}");
    }

    #[test]
    fn the_logo_parts_join_back_to_the_picture() {
        let picture = Picture {
            mime: "image/png".into(),
            data: (0..130_000u32).map(|i| (i % 251) as u8).collect(),
        };
        let messages = brand_messages("Radio Lipa", 7, Some(&picture));
        let head: serde_json::Value = serde_json::from_str(&messages[0]).unwrap();
        assert_eq!(head["type"], "brand");
        assert_eq!(head["name"], "Radio Lipa");
        assert_eq!(head["logo"]["id"], 7);
        assert_eq!(head["logo"]["mime"], "image/png");
        let mut parts: Vec<(u64, Vec<u8>)> = messages[1..]
            .iter()
            .map(|m| {
                let v: serde_json::Value = serde_json::from_str(m).unwrap();
                assert_eq!(v["type"], "logo");
                assert_eq!(v["id"], 7);
                let data = base64::engine::general_purpose::STANDARD
                    .decode(v["data"].as_str().unwrap())
                    .unwrap();
                (v["part"].as_u64().unwrap(), data)
            })
            .collect();
        assert_eq!(
            head["logo"]["parts"].as_u64().unwrap() as usize,
            parts.len()
        );
        assert!(parts.len() > 1, "the picture needs more than one part");
        parts.sort_by_key(|p| p.0);
        let joined: Vec<u8> = parts.into_iter().flat_map(|p| p.1).collect();
        assert_eq!(joined, picture.data);
    }

    #[test]
    fn a_brand_without_a_logo_is_one_message_with_a_null_logo() {
        let messages = brand_messages("Radio Lipa", 1, None);
        assert_eq!(messages.len(), 1);
        let head: serde_json::Value = serde_json::from_str(&messages[0]).unwrap();
        assert_eq!(head["type"], "brand");
        assert!(head["logo"].is_null());
    }

    #[test]
    fn an_image_file_is_its_own_cover() {
        let dir = Scratch::new("cover");
        let solid = DynamicImage::ImageRgb8(RgbImage::new(8, 8));
        let path = dir.file("cover.png", &encode(&solid, ImageFormat::Png));
        let (mime, data) = crate::now::read_cover(&path).unwrap().unwrap();
        assert!(mime.starts_with("image/"));
        assert!(image::load_from_memory(&data).is_ok());
        let bad = dir.file("bad.webp", b"nope");
        assert!(crate::now::read_cover(&bad).is_err());
    }

    #[test]
    fn a_detailed_picture_with_alpha_fits_the_limit() {
        let dir = Scratch::new("noise");
        let mut state = 0x2545_F491_4F6C_DD1Du64;
        let mut noise = RgbaImage::new(700, 700);
        for p in noise.pixels_mut() {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            let b = state.to_le_bytes();
            *p = image::Rgba([b[0], b[1], b[2], b[3] | 1]);
        }
        let path = dir.file("noise.png", &encode_png(&DynamicImage::ImageRgba8(noise)));
        let picture = load_picture(&path).unwrap();
        assert!(picture.data.len() <= 120 * 1024, "{}", picture.data.len());
        assert!(image::load_from_memory(&picture.data).is_ok());
    }

    fn encode_png(image: &DynamicImage) -> Vec<u8> {
        encode(image, ImageFormat::Png)
    }

    /// Sets a brand with a logo that loads when the test says so. Returns the thread and the
    /// sender that releases the loader with its answer.
    fn slow_brand(
        brand: &SharedBrand,
        errors: &PictureErrors,
        name: &str,
    ) -> (
        std::thread::JoinHandle<()>,
        std::sync::mpsc::Sender<Result<Picture, String>>,
    ) {
        let (release, wait) = std::sync::mpsc::channel();
        let thread = set_with(
            brand,
            errors,
            name.into(),
            Some(PathBuf::from("slow.png")),
            move |_| wait.recv().unwrap(),
        )
        .unwrap();
        (thread, release)
    }

    #[test]
    fn a_newer_brand_wins_over_a_slow_one_that_fails() {
        let brand = SharedBrand::default();
        let errors = PictureErrors::default();
        let (thread, release) = slow_brand(&brand, &errors, "First");
        set(&brand, &errors, "Second".into(), None);
        release.send(Err("slow".into())).unwrap();
        thread.join().unwrap();
        let b = brand.lock().unwrap();
        assert_eq!(b.name.as_deref(), Some("Second"));
        assert!(b.logo.is_none());
        assert!(b.messages[0].contains("Second"), "{:?}", b.messages);
        assert!(errors.lock().unwrap().is_empty(), "the old logo's error");
    }

    #[test]
    fn a_newer_brand_wins_over_a_slow_one_that_loads() {
        let brand = SharedBrand::default();
        let errors = PictureErrors::default();
        let (thread, release) = slow_brand(&brand, &errors, "First");
        set(&brand, &errors, "Second".into(), None);
        let old = Picture {
            mime: "image/png".into(),
            data: vec![1, 2, 3],
        };
        release.send(Ok(old)).unwrap();
        thread.join().unwrap();
        let b = brand.lock().unwrap();
        assert_eq!(b.name.as_deref(), Some("Second"));
        assert!(b.logo.is_none(), "the old logo came back");
        assert!(b.messages[0].contains("Second"), "{:?}", b.messages);
    }

    #[test]
    fn a_picture_that_cannot_fit_says_so() {
        let noise = DynamicImage::ImageRgb8(RgbImage::from_fn(300, 300, |x, y| {
            image::Rgb([(x * 7 + y * 13) as u8, (x ^ y) as u8, (x * y) as u8])
        }));
        let reason = fit(&noise, 100).unwrap_err();
        assert_eq!(reason, "the picture is too detailed to send to listeners");
    }

    #[test]
    fn the_error_list_keeps_the_newest() {
        let errors = PictureErrors::default();
        for i in 0..40 {
            record(
                &errors,
                PictureError {
                    path: PathBuf::from(format!("{i}.png")),
                    reason: "no".into(),
                },
            );
        }
        let list = errors.lock().unwrap();
        assert!(list.len() <= 32);
        assert_eq!(list.last().unwrap().path, PathBuf::from("39.png"));
    }
}
