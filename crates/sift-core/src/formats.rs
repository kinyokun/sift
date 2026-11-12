use std::path::Path;

pub const RAW_EXTS: &[&str] = &[
    "cr3", "cr2", "nef", "nefx", "nrw", "arw", "sr2", "srf", "dng", "raf", "orf", "rw2", "pef", "3fr", "iiq", "erf", "srw",
];
pub const IMAGE_EXTS: &[&str] = &["jpg", "jpeg", "png", "webp", "bmp", "gif", "tif", "tiff"];

pub fn ext_of(path: &Path) -> String {
    path.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase()
}

pub fn is_raw(path: &Path) -> bool {
    RAW_EXTS.contains(&ext_of(path).as_str())
}

pub fn is_jpeg(path: &Path) -> bool {
    matches!(ext_of(path).as_str(), "jpg" | "jpeg")
}

pub fn is_supported(path: &Path) -> bool {
    let e = ext_of(path);
    RAW_EXTS.contains(&e.as_str()) || IMAGE_EXTS.contains(&e.as_str())
}

/// Stem used to pair a JPEG with its RAW and to match files across folders.
pub fn stem_key(path: &Path) -> String {
    path.file_stem().and_then(|s| s.to_str()).unwrap_or("").to_ascii_lowercase()
}

/// Like [`stem_key`], but also ignores the colour-space prefix cameras put on
/// file names (`IMG_1234` vs `_MG_1234`, `DSC_1234` vs `_DSC1234`).
pub fn loose_stem_key(path: &Path) -> String {
    let stem = stem_key(path);
    for prefix in ["img_", "_mg_", "dsc_", "_dsc", "dsc"] {
        if let Some(rest) = stem.strip_prefix(prefix) {
            if rest.chars().next().is_some_and(|c| c.is_ascii_digit()) {
                return rest.trim_start_matches('0').to_string();
            }
        }
    }
    stem
}

/// File extension implied by a file's contents, for files that arrive
/// without a usable name (Android document pickers hand out opaque ids).
pub fn sniff_ext(path: &Path) -> Option<&'static str> {
    use crate::container::{parse, Format};
    let src = crate::io::Src::open(path).ok()?;
    let head = src.read_upto(0, 16);
    let p = parse(&src);
    Some(match p.format {
        Format::Jpeg => "jpg",
        Format::Cr3 => "cr3",
        Format::Raf => "raf",
        Format::Tiff => {
            let make = crate::exif::read(&p).make.unwrap_or_default().to_ascii_uppercase();
            match make.split_whitespace().next().unwrap_or("") {
                "CANON" => "cr2",
                "NIKON" => "nef",
                "SONY" => "arw",
                "PENTAX" | "RICOH" => "pef",
                "OLYMPUS" | "OM" => "orf",
                "PANASONIC" => "rw2",
                _ if p.previews.is_empty() => "tif",
                _ => "dng",
            }
        }
        Format::Other => match head.as_slice() {
            [0x89, b'P', b'N', b'G', ..] => "png",
            [b'G', b'I', b'F', ..] => "gif",
            [b'B', b'M', ..] => "bmp",
            [b'R', b'I', b'F', b'F', _, _, _, _, b'W', b'E', b'B', b'P', ..] => "webp",
            _ => return None,
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loose_keys_ignore_colour_space_prefix() {
        let k = |s: &str| loose_stem_key(Path::new(s));
        assert_eq!(k("IMG_1234.JPG"), k("_MG_1234.CR3"));
        assert_eq!(k("DSC01234.JPG"), k("_DSC1234.ARW"));
        assert_eq!(k("DSC_0040.NEF"), k("_DSC0040.jpg"));
        assert_ne!(k("IMG_1234.JPG"), k("IMG_1235.JPG"));
        assert_eq!(k("holiday.jpg"), "holiday");
    }
}
