//! Ratings in Nikon's own sidecars (`NKSC_PARAM/<file name>.nksc`).
//!
//! Nikon's software (NX Studio, Nikon Transfer) keeps a sidecar per photo in
//! a `NKSC_PARAM` folder next to it: an XMP document that holds the rating
//! twice, as `nine:Rating` and inside a base64 copy of the photo's own XMP
//! packet (`ast:XMLPackets`), next to every adjustment made there. A rating
//! written into the photo alone leaves that copy behind, so both are brought
//! in step with the file. Nothing else in the sidecar is touched, and one is
//! never created. The other way round, the stars given in Nikon's software
//! are read from `nine:Rating` when a folder is listed ([`Sidecars`]).

use crate::io::find;
use crate::xmp::{self, Patch};
use std::collections::HashMap;
use std::fs;
use std::ops::Range;
use std::path::{Path, PathBuf};

const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
/// How the sidecar breaks its base64 lines: an escaped line feed.
const BREAK: &[u8] = b"&#xA;";

/// Where Nikon's software keeps the sidecar of `photo`.
pub fn path(photo: &Path) -> Option<PathBuf> {
    let mut name = photo.file_name()?.to_os_string();
    name.push(".nksc");
    Some(photo.parent()?.join("NKSC_PARAM").join(name))
}

/// Brings the rating in the sidecar of `photo` in step, when it has one.
/// Says whether a sidecar was rewritten.
pub fn sync(photo: &Path, rating: u8) -> std::io::Result<bool> {
    let Some(side) = path(photo) else { return Ok(false) };
    let Ok(old) = fs::read(&side) else { return Ok(false) };
    match set_rating(&old, rating) {
        Some(new) if new != old => {
            crate::rating::replace_file(&side, &[&new])?;
            Ok(true)
        }
        _ => Ok(false),
    }
}

/// The rating Nikon's software keeps in a sidecar, when it has given one:
/// none is `None`, like a sidecar without the field.
pub fn rating(doc: &[u8]) -> Option<u8> {
    let range = value(doc, b"<nine:Rating>", b"</nine:Rating>", 0)?;
    let stars = std::str::from_utf8(&doc[range]).ok()?.trim().parse::<u8>().ok()?;
    (1..=5).contains(&stars).then_some(stars)
}

/// The sidecars of the photos of one folder, from one listing of its
/// `NKSC_PARAM`: asking for each photo's on its own is a round trip per
/// photo on a network share.
pub struct Sidecars {
    /// By the photo's name without the extension, in lower case: the photo's name, and its sidecar.
    by_stem: HashMap<String, Vec<(String, PathBuf)>>,
}

impl Sidecars {
    /// `None` for a folder Nikon's software has kept nothing about.
    pub fn of(dir: &Path) -> Option<Sidecars> {
        let mut by_stem: HashMap<String, Vec<(String, PathBuf)>> = HashMap::new();
        for entry in fs::read_dir(dir.join("NKSC_PARAM")).ok()?.flatten() {
            let name = entry.file_name().to_string_lossy().to_lowercase();
            let Some(photo) = name.strip_suffix(".nksc") else { continue };
            let stem = Path::new(photo).file_stem().unwrap_or_default().to_string_lossy().into_owned();
            by_stem.entry(stem).or_default().push((photo.to_string(), entry.path()));
        }
        (!by_stem.is_empty()).then_some(Sidecars { by_stem })
    }

    /// The stars `photo` was given in Nikon's software: in its own sidecar,
    /// else in that of the other file of its pair, which is rated with it there.
    pub fn rating(&self, photo: &Path) -> Option<u8> {
        let name = photo.file_name()?.to_string_lossy().to_lowercase();
        let alike = self.by_stem.get(Path::new(&name).file_stem()?.to_str()?)?;
        let stars = |side: &PathBuf| fs::read(side).ok().and_then(|doc| rating(&doc));
        let own = alike.iter().find(|(photo, _)| *photo == name).and_then(|(_, side)| stars(side));
        own.or_else(|| alike.iter().filter(|(photo, _)| *photo != name).find_map(|(_, side)| stars(side)))
    }
}

/// The text between `open` and the next `close`, looked for from `from`.
fn value(doc: &[u8], open: &[u8], close: &[u8], from: usize) -> Option<Range<usize>> {
    let start = from + find(&doc[from..], open)? + open.len();
    let end = start + find(&doc[start..], close)?;
    Some(start..end)
}

/// The base64 text of the copied XMP packet.
fn packet(doc: &[u8]) -> Option<Range<usize>> {
    let at = find(doc, b"<ast:XMLPackets")?;
    let end = at + find(&doc[at..], b"</ast:XMLPackets>")?;
    value(&doc[..end], b"<rdf:value>", b"</rdf:value>", at)
}

/// The sidecar with `rating` in every place it keeps one, or `None` when it
/// keeps none this module knows.
pub fn set_rating(doc: &[u8], rating: u8) -> Option<Vec<u8>> {
    let digit = [b'0' + rating.min(5)];
    let mut edits: Vec<(Range<usize>, Vec<u8>)> = Vec::new();
    if let Some(range) = packet(doc) {
        if let Some(text) = repacked(&doc[range.clone()], rating) {
            edits.push((range, text));
        }
    }
    if let Some(range) = value(doc, b"<nine:Rating>", b"</nine:Rating>", 0) {
        edits.push((range, digit.to_vec()));
    }
    if edits.is_empty() {
        return None;
    }
    // Back to front, so the ranges found in the original still hold.
    edits.sort_by_key(|(range, _)| std::cmp::Reverse(range.start));
    let mut out = doc.to_vec();
    for (range, text) in edits {
        out.splice(range, text);
    }
    Some(out)
}

/// The base64 text of the packet with the rating set, laid out as it was.
/// `None` leaves the copy alone: when it is not base64, has no place for a
/// rating, or is laid out in a way that would not come back the same.
fn repacked(text: &[u8], rating: u8) -> Option<Vec<u8>> {
    let line = find(text, BREAK).unwrap_or(usize::MAX);
    let old = decode(text)?;
    if encode(&old, line) != text {
        return None;
    }
    match xmp::set_rating(&old, rating)? {
        Patch::Same(new) | Patch::Resized(new) => Some(encode(&new, line)),
    }
}

fn decode(text: &[u8]) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(text.len() / 4 * 3);
    let (mut acc, mut bits, mut i) = (0u32, 0u32, 0);
    while i < text.len() {
        if text[i..].starts_with(BREAK) {
            i += BREAK.len();
            continue;
        }
        let c = text[i];
        i += 1;
        if c == b'=' || c.is_ascii_whitespace() {
            continue;
        }
        let v = ALPHABET.iter().position(|&a| a == c)? as u32;
        acc = (acc << 6) | v;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
        }
    }
    Some(out)
}

/// Base64 with a break after every `line` characters.
fn encode(data: &[u8], line: usize) -> Vec<u8> {
    let mut plain = Vec::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let n = chunk.iter().fold(0u32, |n, &b| (n << 8) | b as u32) << (8 * (3 - chunk.len()));
        for k in 0..4 {
            plain.push(if k <= chunk.len() { ALPHABET[(n >> (18 - 6 * k)) as usize & 63] } else { b'=' });
        }
    }
    if line == 0 || line >= plain.len() {
        return plain;
    }
    let mut out = Vec::with_capacity(plain.len() + plain.len() / line * BREAK.len());
    for (i, piece) in plain.chunks(line).enumerate() {
        if i > 0 {
            out.extend_from_slice(BREAK);
        }
        out.extend_from_slice(piece);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The photo's own packet, as a Nikon camera writes it.
    fn camera_packet(rating: &str) -> Vec<u8> {
        format!(
            "<?xpacket begin=\"\u{feff}\" id=\"W5M0MpCehiHzreSzNTczkc9d\"?><x:xmpmeta xmlns:x=\"adobe:ns:meta/\">\
             <rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\"><rdf:Description rdf:about=\"\" \
             xmlns:xmp=\"http://ns.adobe.com/xap/1.0/\"><xmp:CreatorTool>NIKON Z 8 Ver.01.00   </xmp:CreatorTool>\
             {rating} </rdf:Description></rdf:RDF></x:xmpmeta>{}<?xpacket end=\"w\"?>",
            " ".repeat(300)
        )
        .into_bytes()
    }

    /// A sidecar laid out the way NX Studio 1.10 writes one.
    fn sidecar(packet: &[u8], fields: &str) -> Vec<u8> {
        let packet = String::from_utf8(encode(packet, 76)).unwrap();
        format!(
            "<?xpacket begin=\"\u{feff}\" id=\"W5M0MpCehiHzreSzNTczkc9d\"?>\n\
             <x:xmpmeta xmlns:x=\"adobe:ns:meta/\" x:xmptk=\"XMP Core 5.5.0\">\n   \
             <rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\">\n      \
             <rdf:Description rdf:about=\"core-sidear-tags/1.0\"\n            \
             xmlns:sdc=\"http://ns.nikon.com/sdc/1.0/\"\n            \
             xmlns:ast=\"http://ns.nikon.com/asteroid/1.0/\"\n            \
             xmlns:astype=\"http://ns.nikon.com/asteroid/Types/1.0/\"\n            \
             xmlns:nine=\"http://ns.nikon.com/nine/1.0/\">\n         \
             <sdc:about>nikon sidecar/1.0</sdc:about>\n         \
             <ast:XMLPackets rdf:parseType=\"Resource\">\n            \
             <rdf:value>{packet}</rdf:value>\n            \
             <astype:Type>Binary</astype:Type>\n         \
             </ast:XMLPackets>\n         \
             <ast:GPSVersionID rdf:parseType=\"Resource\">\n            \
             <rdf:value>AgMAAA==</rdf:value>\n            \
             <astype:Type>Binary</astype:Type>\n         \
             </ast:GPSVersionID>\n         \
             <nine:NineEdits>&lt;userData&gt;&lt;filter id=\"nikon::WhiteBalance\"&gt;&lt;/filter&gt;&lt;/userData&gt;</nine:NineEdits>\n         \
             <nine:Label>0</nine:Label>\n{fields}         \
             <nine:Trim>/////////////////////w==</nine:Trim>\n      \
             </rdf:Description>\n   </rdf:RDF>\n</x:xmpmeta>\n\n<?xpacket end=\"w\"?>"
        )
        .into_bytes()
    }

    const RATED: &str = "         <nine:Rating>0</nine:Rating>\n";

    /// What the sidecar says in its two places.
    fn ratings(doc: &[u8]) -> (Option<u8>, Option<u8>) {
        let own = value(doc, b"<nine:Rating>", b"</nine:Rating>", 0)
            .and_then(|r| std::str::from_utf8(&doc[r]).ok()?.parse().ok());
        let copied = packet(doc).and_then(|r| xmp::read_rating(&decode(&doc[r])?));
        (own, copied)
    }

    #[test]
    fn the_stars_given_in_nikons_software_are_read() {
        assert_eq!(rating(&sidecar(&camera_packet("0"), "<nine:Rating>4</nine:Rating>")), Some(4));
        // No stars, no field, and a number that is not a rating all say nothing.
        assert_eq!(rating(&sidecar(&camera_packet("3"), "<nine:Rating>0</nine:Rating>")), None);
        assert_eq!(rating(&sidecar(&camera_packet("3"), "<nine:Label>2</nine:Label>")), None);
        assert_eq!(rating(&sidecar(&camera_packet("3"), "<nine:Rating>9</nine:Rating>")), None);

        let dir = std::env::temp_dir().join(format!("sift-nksc-read-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        assert!(Sidecars::of(&dir).is_none());
        fs::create_dir_all(dir.join("NKSC_PARAM")).unwrap();
        let keep = |photo: &str, stars: &str| fs::write(dir.join("NKSC_PARAM").join(format!("{photo}.nksc")), sidecar(&camera_packet("0"), &format!("<nine:Rating>{stars}</nine:Rating>"))).unwrap();
        keep("DSC_0001.NEF", "5");
        keep("DSC_0001.JPG", "0");
        keep("DSC_0002.NEF", "2");
        keep("DSC_0002.JPG", "3");
        keep("DSC_0003.NEF", "0");
        let sides = Sidecars::of(&dir).unwrap();
        // The file's own sidecar first, then its pair's; names are matched without regard to case.
        assert_eq!(sides.rating(&dir.join("DSC_0001.JPG")), Some(5));
        assert_eq!(sides.rating(&dir.join("dsc_0002.jpg")), Some(3));
        assert_eq!(sides.rating(&dir.join("DSC_0002.NEF")), Some(2));
        assert_eq!(sides.rating(&dir.join("DSC_0003.NEF")), None);
        assert_eq!(sides.rating(&dir.join("DSC_0004.NEF")), None);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn base64_round_trips_at_every_length() {
        for len in 0..200 {
            let data: Vec<u8> = (0..len).map(|i| (i * 37 + 11) as u8).collect();
            assert_eq!(decode(&encode(&data, 76)).unwrap(), data);
            assert_eq!(decode(&encode(&data, usize::MAX)).unwrap(), data);
        }
        assert_eq!(encode(b"Sift", usize::MAX), b"U2lmdA==");
        assert_eq!(encode(&[0; 60], 76).len(), 80 + BREAK.len());
    }

    #[test]
    fn the_rating_changes_in_both_places_and_nowhere_else() {
        let old = sidecar(&camera_packet("<xmp:Rating>0</xmp:Rating>"), RATED);
        assert_eq!(ratings(&old), (Some(0), Some(0)));
        let new = set_rating(&old, 4).unwrap();
        assert_eq!(ratings(&new), (Some(4), Some(4)));
        assert_eq!(new.len(), old.len());

        let (a, b) = (packet(&old).unwrap(), packet(&new).unwrap());
        assert_eq!(a, b);
        assert_eq!(old[..a.start], new[..b.start]);
        let (tail_old, tail_new) = (&old[a.end..], &new[b.end..]);
        let differing: Vec<usize> = (0..tail_old.len()).filter(|&i| tail_old[i] != tail_new[i]).collect();
        assert_eq!(differing.len(), 1, "only the digit of nine:Rating");
        // One character of the camera's packet, so at most two of its base64.
        let changed = (a.start..a.end).filter(|&i| old[i] != new[i]).count();
        assert!((1..=2).contains(&changed), "{changed} base64 characters changed");

        assert_eq!(set_rating(&new, 0).unwrap(), old);
    }

    #[test]
    fn the_same_rating_leaves_every_byte() {
        let old = sidecar(&camera_packet("<xmp:Rating>3</xmp:Rating>"), "         <nine:Rating>3</nine:Rating>\n");
        assert_eq!(set_rating(&old, 3).unwrap(), old);
    }

    #[test]
    fn a_packet_without_a_rating_gets_one_in_its_padding() {
        let old = sidecar(&camera_packet(""), RATED);
        let new = set_rating(&old, 5).unwrap();
        assert_eq!(ratings(&new), (Some(5), Some(5)));
        assert_eq!(new.len(), old.len());
    }

    #[test]
    fn a_copy_that_cannot_be_read_is_left_alone() {
        let good = sidecar(&camera_packet("<xmp:Rating>0</xmp:Rating>"), RATED);
        let range = packet(&good).unwrap();
        // Not base64.
        let mut odd = good.clone();
        odd[range.start] = b'*';
        let new = set_rating(&odd, 2).unwrap();
        assert_eq!(new[range.clone()], odd[range.clone()]);
        assert_eq!(ratings(&new).0, Some(2));
        // Base64 broken into lines some other way.
        let mut odd = good.clone();
        odd.splice(range.start + 10..range.start + 10, *b"\n");
        let new = set_rating(&odd, 2).unwrap();
        assert_eq!(new[range.start..range.end + 1], odd[range.start..range.end + 1]);
        assert_eq!(ratings(&new).0, Some(2));
    }

    #[test]
    fn a_sidecar_without_a_rating_is_not_given_one() {
        let mut doc = sidecar(b"<x:xmpmeta xmlns:x=\"adobe:ns:meta/\"></x:xmpmeta>", "");
        assert!(set_rating(&doc, 3).is_none());
        doc.truncate(40);
        assert!(set_rating(&doc, 3).is_none());
        assert!(set_rating(b"", 3).is_none());
    }

    #[test]
    fn only_a_sidecar_that_exists_is_written() {
        let dir = std::env::temp_dir().join(format!("sift-nksc-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("NKSC_PARAM")).unwrap();
        let photo = dir.join("DSC_0001.NEF");
        let side = dir.join("NKSC_PARAM").join("DSC_0001.NEF.nksc");
        assert_eq!(path(&photo).unwrap(), side);

        assert!(!sync(&photo, 4).unwrap());
        assert_eq!(fs::read_dir(dir.join("NKSC_PARAM")).unwrap().count(), 0);

        let old = sidecar(&camera_packet("<xmp:Rating>0</xmp:Rating>"), RATED);
        fs::write(&side, &old).unwrap();
        assert!(sync(&photo, 4).unwrap());
        assert_eq!(ratings(&fs::read(&side).unwrap()), (Some(4), Some(4)));
        assert!(!sync(&photo, 4).unwrap(), "nothing to change the second time");
        assert_eq!(fs::read_dir(dir.join("NKSC_PARAM")).unwrap().count(), 1, "no temporary file left");
        assert!(!sync(&dir.join("DSC_0002.NEF"), 4).unwrap());
        fs::remove_dir_all(&dir).unwrap();
    }
}
