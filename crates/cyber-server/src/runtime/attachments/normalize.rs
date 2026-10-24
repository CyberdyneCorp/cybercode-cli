//! Deterministic normalization of one attachment (`session-runtime` → Attachment
//! normalization): images over `image.max_dimension` are downscaled, images over
//! `image.max_bytes` are re-encoded as JPEG quality 85, and a PDF's pages are counted for
//! the page note. Decoding is bounded before any pixel buffer is allocated.

use std::io::Cursor;

use cyber_core::config::AttachmentSettings;
use image::{DynamicImage, ImageFormat, ImageReader, Limits};

use super::{AttachmentError, sniff};

/// Images above this many pixels are refused before decoding.
const MAX_PIXELS: u64 = 100_000_000;
/// Decoder allocation ceiling.
const MAX_ALLOC: u64 = 512 * 1024 * 1024;
const JPEG_QUALITY: u8 = 85;
/// No PDF object or cross-reference stream may inflate past this while its pages are
/// counted.
const MAX_PDF_STREAM: usize = 4 * 1024 * 1024;

/// The bytes to store for one attachment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Normalized {
    pub bytes: Vec<u8>,
    pub media_type: &'static str,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub pages: Option<u32>,
    /// Text the model gets after the attachment (a long PDF's page range).
    pub note: Option<String>,
    /// The original bytes, with `keep_original`, when normalization changed them.
    pub original: Option<Vec<u8>>,
}

/// Sniff and normalize `bytes`, already checked against `attachments.max_bytes`.
pub fn normalize(
    bytes: Vec<u8>,
    settings: &AttachmentSettings,
    name: Option<&str>,
) -> Result<Normalized, AttachmentError> {
    let media_type = sniff(&bytes).ok_or_else(|| AttachmentError::Unsupported {
        media_type: "application/octet-stream".into(),
        reason: "Unsupported attachment: attach a PNG, JPEG, GIF or WebP image or a PDF".into(),
    })?;
    if media_type == "application/pdf" {
        return Ok(pdf(bytes, settings, name));
    }
    image(bytes, media_type, settings)
}

/// A PDF's page count: the root page tree's `/Count`, or `None` when it cannot be read
/// within the bounds. Loading keeps only the catalog, page tree nodes and object
/// streams, and no stream may inflate past [`MAX_PDF_STREAM`], so a decompression bomb
/// costs bounded memory.
fn page_count(bytes: &[u8]) -> Option<u32> {
    let options = lopdf::LoadOptions {
        filter: Some(page_tree_only),
        max_decompressed_size: Some(MAX_PDF_STREAM),
        ..lopdf::LoadOptions::default()
    };
    let document = lopdf::Document::load_mem_with_options(bytes, options).ok()?;
    let pages = document
        .catalog()
        .ok()?
        .get(b"Pages")
        .ok()?
        .as_reference()
        .ok()?;
    let count = document.get_dictionary(pages).ok()?.get(b"Count").ok()?;
    u32::try_from(count.as_i64().ok()?).ok()
}

fn page_tree_only(
    id: lopdf::ObjectId,
    object: &mut lopdf::Object,
) -> Option<(lopdf::ObjectId, lopdf::Object)> {
    let keep = match &*object {
        lopdf::Object::Stream(stream) => stream.dict.has_type(b"ObjStm"),
        lopdf::Object::Dictionary(dict) => dict.has_type(b"Catalog") || dict.has_type(b"Pages"),
        _ => false,
    };
    // lopdf keeps `object` itself for top-level objects and the returned copy for those
    // inside an object stream.
    keep.then(|| (id, object.clone()))
}

fn pdf(bytes: Vec<u8>, settings: &AttachmentSettings, name: Option<&str>) -> Normalized {
    // A PDF whose page count cannot be read is attached as is; the provider's own limit
    // still applies.
    let pages = page_count(&bytes);
    let max = settings.pdf_max_pages;
    let note = pages.filter(|n| *n > max).map(|n| {
        let name = name.unwrap_or("document.pdf");
        format!(
            "[{name}: {n} pages; only pages 1-{max} are expected to be read unless the user names others]"
        )
    });
    Normalized {
        bytes,
        media_type: "application/pdf",
        width: None,
        height: None,
        pages,
        note,
        original: None,
    }
}

fn unsupported(media_type: &str, reason: impl Into<String>) -> AttachmentError {
    AttachmentError::Unsupported {
        media_type: media_type.into(),
        reason: reason.into(),
    }
}

fn reader(bytes: &[u8], format: ImageFormat) -> ImageReader<Cursor<&[u8]>> {
    let mut limits = Limits::default();
    limits.max_alloc = Some(MAX_ALLOC);
    let mut reader = ImageReader::with_format(Cursor::new(bytes), format);
    reader.limits(limits);
    reader
}

fn image(
    bytes: Vec<u8>,
    media_type: &'static str,
    settings: &AttachmentSettings,
) -> Result<Normalized, AttachmentError> {
    let format = ImageFormat::from_mime_type(media_type).expect("sniffed image types decode");
    // Only the header is read here, so a decompression bomb is refused before its pixels.
    let (width, height) = reader(&bytes, format)
        .into_dimensions()
        .map_err(|error| unsupported(media_type, format!("Unreadable image: {error}")))?;
    if u64::from(width) * u64::from(height) > MAX_PIXELS {
        return Err(unsupported(
            media_type,
            format!("Image is {width}x{height}; the limit is 100 megapixels"),
        ));
    }
    let max_dimension = settings.image_max_dimension;
    let fits = width.max(height) <= max_dimension;
    if fits && bytes.len() as u64 <= settings.image_max_bytes {
        // Within both limits (an animated GIF too): kept byte for byte.
        return Ok(described(bytes, media_type, width, height));
    }
    let mut decoded = reader(&bytes, format)
        .decode()
        .map_err(|error| unsupported(media_type, format!("Unreadable image: {error}")))?;
    if !fits {
        decoded = decoded.resize(
            max_dimension,
            max_dimension,
            image::imageops::FilterType::Lanczos3,
        );
    }
    let (encoded, encoded_type) = encode(&decoded, settings.image_max_bytes)?;
    let mut normalized = described(encoded, encoded_type, decoded.width(), decoded.height());
    normalized.original = settings.keep_original.then_some(bytes);
    Ok(normalized)
}

/// PNG, or JPEG quality 85 when the PNG is over `max_bytes`.
fn encode(
    image: &DynamicImage,
    max_bytes: u64,
) -> Result<(Vec<u8>, &'static str), AttachmentError> {
    let failed = |error: image::ImageError| AttachmentError::Storage(error.to_string());
    let mut png = Vec::new();
    image
        .write_to(&mut Cursor::new(&mut png), ImageFormat::Png)
        .map_err(failed)?;
    if png.len() as u64 <= max_bytes {
        return Ok((png, "image/png"));
    }
    let mut jpeg = Vec::new();
    let encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpeg, JPEG_QUALITY);
    image
        .to_rgb8()
        .write_with_encoder(encoder)
        .map_err(failed)?;
    Ok((jpeg, "image/jpeg"))
}

fn described(bytes: Vec<u8>, media_type: &'static str, width: u32, height: u32) -> Normalized {
    Normalized {
        bytes,
        media_type,
        width: Some(width),
        height: Some(height),
        pages: None,
        note: None,
        original: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A noisy image, so its PNG is large (a photo, not a flat screenshot).
    fn noisy_png(width: u32, height: u32) -> Vec<u8> {
        let mut state = 0x2545_f491_u32;
        let image = image::RgbImage::from_fn(width, height, |_, _| {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            let [r, g, b, _] = state.to_le_bytes();
            image::Rgb([r, g, b])
        });
        let mut bytes = Vec::new();
        image
            .write_to(&mut Cursor::new(&mut bytes), ImageFormat::Png)
            .unwrap();
        bytes
    }

    fn pdf_with_pages(count: usize) -> Vec<u8> {
        use lopdf::{Document, Object, dictionary};
        let mut document = Document::with_version("1.5");
        let pages_id = document.new_object_id();
        let kids: Vec<Object> = (0..count)
            .map(|_| {
                document
                    .add_object(dictionary! {
                        "Type" => "Page",
                        "Parent" => pages_id,
                        "MediaBox" => vec![0.into(), 0.into(), 612.into(), 792.into()],
                    })
                    .into()
            })
            .collect();
        document.objects.insert(
            pages_id,
            Object::Dictionary(dictionary! {
                "Type" => "Pages",
                "Kids" => kids,
                "Count" => count as i64,
            }),
        );
        let catalog = document.add_object(dictionary! { "Type" => "Catalog", "Pages" => pages_id });
        document.trailer.set("Root", catalog);
        let mut bytes = Vec::new();
        document.save_to(&mut bytes).unwrap();
        bytes
    }

    #[test]
    fn a_large_screenshot_is_downscaled_and_kept_under_the_byte_limit() {
        let settings = AttachmentSettings::default();
        let original = noisy_png(4000, 3000);
        assert!(original.len() as u64 > settings.image_max_bytes);
        let normalized = normalize(original.clone(), &settings, Some("shot.png")).unwrap();
        assert_eq!(
            (normalized.width, normalized.height),
            (Some(2000), Some(1500))
        );
        assert!(normalized.bytes.len() as u64 <= settings.image_max_bytes);
        // The noisy PNG stays over 5 MiB at 2000 px, so it becomes JPEG quality 85.
        assert_eq!(normalized.media_type, "image/jpeg");
        assert_eq!(sniff(&normalized.bytes), Some("image/jpeg"));
        assert_eq!(normalized.original, None);
        // Deterministic: the same input gives the same blob.
        assert_eq!(
            normalize(original.clone(), &settings, None).unwrap().bytes,
            normalized.bytes
        );
        let keep = AttachmentSettings {
            keep_original: true,
            ..settings
        };
        assert_eq!(
            normalize(original.clone(), &keep, None).unwrap().original,
            Some(original)
        );
    }

    #[test]
    fn an_image_within_the_limits_is_kept_byte_for_byte() {
        let small = noisy_png(40, 30);
        let normalized = normalize(small.clone(), &AttachmentSettings::default(), None).unwrap();
        assert_eq!(normalized.bytes, small);
        assert_eq!(normalized.media_type, "image/png");
        assert_eq!((normalized.width, normalized.height), (Some(40), Some(30)));
    }

    #[test]
    fn a_decompression_bomb_header_is_refused_before_decoding() {
        // A valid PNG claiming 50000x50000 pixels (2.5 gigapixels) with a tiny IDAT, the
        // shape of a decompression bomb.
        let ihdr = [
            50_000u32.to_be_bytes().as_slice(),
            50_000u32.to_be_bytes().as_slice(),
            &[8, 2, 0, 0, 0],
        ]
        .concat();
        let bomb = [
            b"\x89PNG\r\n\x1a\n".to_vec(),
            chunk(b"IHDR", &ihdr),
            chunk(b"IDAT", &[0x78, 0x9c, 0x03, 0x00, 0x00, 0x00, 0x00, 0x01]),
            chunk(b"IEND", &[]),
        ]
        .concat();
        let error = normalize(bomb, &AttachmentSettings::default(), None).unwrap_err();
        let AttachmentError::Unsupported { media_type, reason } = error else {
            panic!("{error:?}");
        };
        assert_eq!(media_type, "image/png");
        assert!(reason.contains("100 megapixels"), "{reason}");
    }

    fn chunk(kind: &[u8; 4], data: &[u8]) -> Vec<u8> {
        let length = u32::try_from(data.len()).unwrap().to_be_bytes();
        let crc = crc32(&[kind.as_slice(), data].concat()).to_be_bytes();
        [length.as_slice(), kind, data, &crc].concat()
    }

    fn crc32(bytes: &[u8]) -> u32 {
        let mut crc = !0u32;
        for byte in bytes {
            crc ^= u32::from(*byte);
            for _ in 0..8 {
                crc = if crc & 1 == 1 {
                    (crc >> 1) ^ 0xedb8_8320
                } else {
                    crc >> 1
                };
            }
        }
        !crc
    }

    #[test]
    fn a_long_pdf_gets_the_page_note() {
        let settings = AttachmentSettings::default();
        let long = normalize(pdf_with_pages(240), &settings, Some("spec.pdf")).unwrap();
        assert_eq!(long.media_type, "application/pdf");
        assert_eq!(long.pages, Some(240));
        assert_eq!(
            long.note.as_deref(),
            Some(
                "[spec.pdf: 240 pages; only pages 1-100 are expected to be read unless the user names others]"
            )
        );
        let short = normalize(pdf_with_pages(3), &settings, None).unwrap();
        assert_eq!((short.pages, short.note), (Some(3), None));
        // An unparsable PDF is attached as is, without a note.
        let broken = normalize(b"%PDF-1.7 garbage".to_vec(), &settings, None).unwrap();
        assert_eq!((broken.pages, broken.note), (None, None));
    }

    #[test]
    fn a_pdf_stream_inflating_past_the_bound_is_not_decoded() {
        use lopdf::{Document, Object, dictionary};
        // The page tree sits in an object stream together with a string that inflates
        // past `MAX_PDF_STREAM`: the stream is refused, so the count is unknown.
        let mut document = Document::with_version("1.5");
        let pages_id = document.new_object_id();
        let page = document.add_object(dictionary! { "Type" => "Page", "Parent" => pages_id });
        document.objects.insert(
            pages_id,
            Object::Dictionary(dictionary! {
                "Type" => "Pages", "Kids" => vec![page.into()], "Count" => 1,
            }),
        );
        let catalog = document.add_object(dictionary! { "Type" => "Catalog", "Pages" => pages_id });
        document.trailer.set("Root", catalog);
        let mut modern = Vec::new();
        document.save_modern(&mut modern).unwrap();
        let settings = AttachmentSettings::default();
        assert_eq!(normalize(modern, &settings, None).unwrap().pages, Some(1));
        let filler = Object::string_literal(vec![b'a'; MAX_PDF_STREAM + 1]);
        document.add_object(filler);
        let mut bomb = Vec::new();
        document.save_modern(&mut bomb).unwrap();
        assert!(bomb.len() < MAX_PDF_STREAM / 16, "{} bytes", bomb.len());
        let unbounded = Document::load_mem(&bomb).unwrap();
        assert_eq!(
            unbounded.get_pages().len(),
            1,
            "an unbounded load inflates it"
        );
        let normalized = normalize(bomb.clone(), &settings, None).unwrap();
        assert_eq!(
            (normalized.pages, normalized.note, normalized.bytes),
            (None, None, bomb)
        );
    }

    #[test]
    fn other_bytes_are_unsupported_whatever_their_name() {
        let error = normalize(
            b"PK\x03\x04zip".to_vec(),
            &AttachmentSettings::default(),
            Some("photo.png"),
        )
        .unwrap_err();
        assert!(
            matches!(error, AttachmentError::Unsupported { ref media_type, .. } if media_type == "application/octet-stream")
        );
    }
}
