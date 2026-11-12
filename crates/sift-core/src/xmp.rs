//! `xmp:Rating` reading and length-preserving rewriting of XMP packets.

use crate::io::{find, rfind};

const NS_XMP: &str = "http://ns.adobe.com/xap/1.0/";

pub enum Patch {
    /// Same byte length as the input: safe to overwrite in place.
    Same(Vec<u8>),
    /// The packet had to grow or shrink; the container must be rewritten.
    Resized(Vec<u8>),
}

/// Byte range of the rating value and the position right after its closing
/// quote or tag (where padding may be inserted).
fn locate(xmp: &[u8]) -> Option<(usize, usize, usize)> {
    let mut from = 0;
    while let Some(i) = find(&xmp[from..], b"xmp:Rating") {
        let at = from + i;
        let mut j = at + b"xmp:Rating".len();
        from = j;
        if at > 0 && xmp[at - 1] == b'<' && xmp.get(j) == Some(&b'>') {
            let start = j + 1;
            let end = start + find(&xmp[start..], b"</xmp:Rating>")?;
            return Some((start, end, end + b"</xmp:Rating>".len()));
        }
        while xmp.get(j).is_some_and(u8::is_ascii_whitespace) {
            j += 1;
        }
        if xmp.get(j) != Some(&b'=') {
            continue;
        }
        j += 1;
        while xmp.get(j).is_some_and(u8::is_ascii_whitespace) {
            j += 1;
        }
        let quote = *xmp.get(j)?;
        if quote != b'"' && quote != b'\'' {
            continue;
        }
        let start = j + 1;
        let end = start + xmp[start..].iter().position(|&b| b == quote)?;
        return Some((start, end, end + 1));
    }
    None
}

/// Rating stored in a packet, clamped to 0..=5 (negative "rejected" reads as 0).
pub fn read_rating(xmp: &[u8]) -> Option<u8> {
    let (s, e, _) = locate(xmp)?;
    let v: f32 = std::str::from_utf8(&xmp[s..e]).ok()?.trim().parse().ok()?;
    Some(v.round().clamp(0.0, 5.0) as u8)
}

/// Restores the original length by trimming or adding whitespace padding.
fn refit(mut out: Vec<u8>, target: usize, pad_at: usize) -> Patch {
    if out.len() < target {
        let fill = vec![b' '; target - out.len()];
        out.splice(pad_at..pad_at, fill);
        return Patch::Same(out);
    }
    let excess = out.len() - target;
    if excess == 0 {
        return Patch::Same(out);
    }
    // XMP packets end with whitespace padding before the trailer for exactly this purpose.
    if let Some(trailer) = rfind(&out, b"<?xpacket end") {
        let pad = out[..trailer].iter().rev().take_while(|b| b.is_ascii_whitespace()).count();
        if pad > excess {
            out.drain(trailer - excess..trailer);
            return Patch::Same(out);
        }
    }
    Patch::Resized(out)
}

pub fn set_rating(xmp: &[u8], rating: u8) -> Option<Patch> {
    let digit = [b'0' + rating.min(5)];
    if let Some((s, e, after)) = locate(xmp) {
        let mut out = xmp.to_vec();
        out.splice(s..e, digit);
        let pad_at = after + 1 - (e - s);
        return Some(refit(out, xmp.len(), pad_at));
    }
    // No rating yet: add the attribute to the first rdf:Description.
    let d = find(xmp, b"<rdf:Description")?;
    let close = d + xmp[d..].iter().position(|&b| b == b'>')?;
    let at = if xmp[close - 1] == b'/' { close - 1 } else { close };
    let mut add = String::new();
    if find(&xmp[..close], b"xmlns:xmp=").is_none() {
        add.push_str(&format!(" xmlns:xmp=\"{NS_XMP}\""));
    }
    add.push_str(&format!(" xmp:Rating=\"{}\"", rating.min(5)));
    let mut out = xmp.to_vec();
    out.splice(at..at, add.bytes());
    Some(refit(out, xmp.len(), at))
}

/// A standalone packet, used for sidecars and for files that have no XMP yet.
pub fn minimal_packet(rating: u8) -> Vec<u8> {
    format!(
        "<?xpacket begin=\"\u{feff}\" id=\"W5M0MpCehiHzreSzNTczkc9d\"?>\n\
         <x:xmpmeta xmlns:x=\"adobe:ns:meta/\" x:xmptk=\"Sift\">\n \
         <rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\">\n  \
         <rdf:Description rdf:about=\"\" xmlns:xmp=\"{NS_XMP}\" xmp:Rating=\"{}\"/>\n \
         </rdf:RDF>\n</x:xmpmeta>\n<?xpacket end=\"w\"?>",
        rating.min(5)
    )
    .into_bytes()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn same(p: Option<Patch>) -> Vec<u8> {
        match p {
            Some(Patch::Same(v)) => v,
            _ => panic!("expected an in-place patch"),
        }
    }

    #[test]
    fn replaces_attribute_and_element_in_place() {
        let attr = b"<rdf:Description xmp:Rating=\"2\" a=\"b\"/>";
        let out = same(set_rating(attr, 5));
        assert_eq!(out.len(), attr.len());
        assert_eq!(read_rating(&out), Some(5));

        let elem = b"<rdf:Description><xmp:Rating>0</xmp:Rating></rdf:Description>";
        let out = same(set_rating(elem, 3));
        assert_eq!(out, b"<rdf:Description><xmp:Rating>3</xmp:Rating></rdf:Description>");
    }

    #[test]
    fn shorter_value_is_padded_outside_the_value() {
        let src = b"<rdf:Description xmp:Rating=\"-1\"/>";
        let out = same(set_rating(src, 4));
        assert_eq!(out, b"<rdf:Description xmp:Rating=\"4\" />");
    }

    #[test]
    fn inserts_using_packet_padding() {
        let src = format!(
            "<x:xmpmeta xmlns:x=\"adobe:ns:meta/\"><rdf:RDF><rdf:Description rdf:about=\"\" xmlns:xmp=\"{NS_XMP}\"/></rdf:RDF></x:xmpmeta>{}<?xpacket end=\"w\"?>",
            " ".repeat(64)
        );
        let out = same(set_rating(src.as_bytes(), 1));
        assert_eq!(out.len(), src.len());
        assert_eq!(read_rating(&out), Some(1));
        assert!(out.ends_with(b" <?xpacket end=\"w\"?>"));
    }

    #[test]
    fn grows_when_there_is_no_padding() {
        let src = b"<rdf:RDF><rdf:Description rdf:about=\"\"></rdf:Description></rdf:RDF>";
        match set_rating(src, 2) {
            Some(Patch::Resized(v)) => {
                assert_eq!(read_rating(&v), Some(2));
                assert!(find(&v, b"xmlns:xmp=").is_some());
            }
            _ => panic!("expected a resized packet"),
        }
    }

    #[test]
    fn minimal_packet_round_trips() {
        assert_eq!(read_rating(&minimal_packet(4)), Some(4));
    }
}
