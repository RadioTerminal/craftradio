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

const NOT_A_PICTURE: &str = "the file is not a PNG, JPEG or WebP picture";

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
    let generation = {
        let Ok(mut b) = brand.lock() else { return };
        b.generation += 1;
        b.name = Some(name.clone());
        b.generation
    };
    let Some(path) = logo else {
        store(brand, generation, name, None);
        return;
    };
    let brand = brand.clone();
    let errors = errors.clone();
    let _ = std::thread::Builder::new()
        .name("webrtc-brand".into())
        .spawn(move || {
            let picture = match load_picture(&path) {
                Ok(p) => Some(p),
                Err(reason) => {
                    if let Ok(mut list) = errors.lock() {
                        list.push(PictureError { path, reason });
                    }
                    None
                }
            };
            store(&brand, generation, name, picture);
        });
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
    let mut picture = image::load_from_memory_with_format(&bytes, format)
        .map_err(|_| NOT_A_PICTURE.to_owned())?;
    if picture.width() > MAX_EDGE || picture.height() > MAX_EDGE {
        picture = picture.resize(MAX_EDGE, MAX_EDGE, image::imageops::FilterType::Lanczos3);
    }
    let mut data = Vec::new();
    let mime = if picture.color().has_alpha() {
        picture
            .to_rgba8()
            .write_to(
                &mut std::io::Cursor::new(&mut data),
                image::ImageFormat::Png,
            )
            .map_err(|_| "the picture could not be encoded".to_owned())?;
        "image/png"
    } else {
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut data, JPEG_QUALITY)
            .encode_image(&picture.to_rgb8())
            .map_err(|_| "the picture could not be encoded".to_owned())?;
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
}
