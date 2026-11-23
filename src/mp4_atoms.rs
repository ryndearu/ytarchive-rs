//! Top-level MP4 box ("atom") scanning and removal. ffmpeg rejects
//! concatenated fragments that repeat `sidx` (and, since 6.1, `ftyp`), so
//! those are stripped before writing.

use std::borrow::Cow;

use bytes::Bytes;

/// Offset and length of each top-level box, in file order.
pub fn atoms(data: &[u8]) -> Vec<([u8; 4], usize, usize)> {
    let mut out = Vec::new();
    let mut ofs = 0usize;
    while let Some((size, name)) =
        ofs.checked_add(8).and_then(|end| data.get(ofs..end)).and_then(|header| header.split_first_chunk::<4>())
    {
        let Ok(tag) = <[u8; 4]>::try_from(name) else {
            break;
        };
        let len = u32::from_be_bytes(*size) as usize;
        // len 0 ("to end of file") and len 1 (64-bit size) aren't used in
        // these fragments; stop rather than loop forever as Go could.
        let Some(next) = ofs.checked_add(len) else {
            break;
        };
        if len < 8 || next > data.len() {
            break;
        }
        out.push((tag, ofs, len));
        ofs = next;
    }
    out
}

/// If only contiguous leading boxes need removal, returns the slice start
/// offset after them; returns `None` when a middle/trailing box must be dropped.
fn leading_strip_offset(found: &[([u8; 4], usize, usize)], names: &[[u8; 4]]) -> Option<usize> {
    let mut lead = 0usize;
    let mut seen_kept = false;
    for (tag, ofs, len) in found {
        if names.contains(tag) {
            if !seen_kept && *ofs == lead {
                lead = *ofs + *len;
            } else {
                return None;
            }
        } else {
            seen_kept = true;
        }
    }
    Some(lead)
}

fn copy_without(data: &[u8], found: &[([u8; 4], usize, usize)], names: &[[u8; 4]]) -> Vec<u8> {
    let mut out = Vec::with_capacity(data.len());
    let mut pos = 0usize;
    for (tag, ofs, len) in found {
        if names.contains(tag) {
            out.extend_from_slice(data.get(pos..*ofs).unwrap_or_default());
            pos = *ofs + *len;
        }
    }
    out.extend_from_slice(data.get(pos..).unwrap_or_default());
    out
}

/// Returns `data` without the named top-level boxes, borrowing when only leading
/// boxes (or none) are removed.
pub fn remove_atoms<'a>(data: &'a [u8], names: &[[u8; 4]]) -> Cow<'a, [u8]> {
    let found = atoms(data);
    if let Some(start) = leading_strip_offset(&found, names) {
        return Cow::Borrowed(data.get(start..).unwrap_or_default());
    }
    Cow::Owned(copy_without(data, &found, names))
}

const FIRST_TAGS: &[[u8; 4]] = &[*b"sidx"];
const OTHER_TAGS: &[[u8; 4]] = &[*b"sidx", *b"ftyp"];

fn strip_tags(first: bool) -> &'static [[u8; 4]] {
    if first { FIRST_TAGS } else { OTHER_TAGS }
}

/// Boxes to strip from a fragment: `sidx` always, `ftyp` except on the
/// first fragment written. Non-MP4 fragments are borrowed unchanged.
pub fn strip_for_concat<'a>(data: &'a [u8], mime_type: &str, first: bool) -> Cow<'a, [u8]> {
    // A blank MIME type is assumed to be MP4.
    if !(mime_type.ends_with("/mp4") || mime_type.is_empty()) {
        return Cow::Borrowed(data);
    }
    remove_atoms(data, strip_tags(first))
}

/// Zero-copy variant of `strip_for_concat` for reference-counted `Bytes`
/// buffers: slices `data` when only leading boxes (or none) are removed.
pub fn strip_bytes(data: Bytes, mime_type: &str, first: bool) -> Bytes {
    if !(mime_type.ends_with("/mp4") || mime_type.is_empty()) {
        return data;
    }
    let names = strip_tags(first);
    let found = atoms(&data);
    if let Some(start) = leading_strip_offset(&found, names) {
        return if start == 0 { data } else { data.slice(start..) };
    }
    Bytes::from(copy_without(&data, &found, names))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mp4_box_bytes(name: &[u8; 4], payload: &[u8]) -> Vec<u8> {
        let mut bytes = ((payload.len() + 8) as u32).to_be_bytes().to_vec();
        bytes.extend_from_slice(name);
        bytes.extend_from_slice(payload);
        bytes
    }

    fn mp4_box(name: &str, payload: &[u8]) -> Vec<u8> {
        mp4_box_bytes(name.as_bytes().try_into().unwrap(), payload)
    }

    fn sample() -> Vec<u8> {
        [mp4_box("ftyp", b"iso6"), mp4_box("sidx", b"xx"), mp4_box("moof", b"m"), mp4_box("mdat", b"data")].concat()
    }

    #[test]
    fn removes_boxes() {
        let data = sample();
        let stripped = strip_for_concat(&data, "video/mp4", false);
        assert!(matches!(stripped, Cow::Borrowed(_)));
        assert_eq!(stripped.as_ref(), [mp4_box("moof", b"m"), mp4_box("mdat", b"data")].concat());

        let first_stripped = strip_for_concat(&data, "", true);
        assert!(matches!(first_stripped, Cow::Owned(_)));
        assert_eq!(
            first_stripped.as_ref(),
            [mp4_box("ftyp", b"iso6"), mp4_box("moof", b"m"), mp4_box("mdat", b"data")].concat()
        );

        let webm = strip_for_concat(&data, "video/webm", false);
        assert!(matches!(webm, Cow::Borrowed(_)));
        assert_eq!(webm.as_ref(), data.as_slice());

        let leading = strip_bytes(Bytes::from(data.clone()), "video/mp4", false);
        assert_eq!(leading.as_ref(), [mp4_box("moof", b"m"), mp4_box("mdat", b"data")].concat());
        let first_bytes = strip_bytes(Bytes::from(data.clone()), "", true);
        assert_eq!(
            first_bytes.as_ref(),
            [mp4_box("ftyp", b"iso6"), mp4_box("moof", b"m"), mp4_box("mdat", b"data")].concat()
        );
        assert_eq!(strip_bytes(Bytes::from(data.clone()), "video/webm", false).as_ref(), data.as_slice());
    }

    #[test]
    fn malformed_input_is_left_alone() {
        let zero_len = [0u8, 0, 0, 0, b's', b'i', b'd', b'x', 1, 2, 3];
        assert_eq!(remove_atoms(&zero_len, &[*b"sidx"]).as_ref(), zero_len);
        let truncated = [0u8, 0, 0, 99, b's', b'i', b'd', b'x'];
        assert_eq!(remove_atoms(&truncated, &[*b"sidx"]).as_ref(), truncated);
        assert!(remove_atoms(&[], &[*b"sidx"]).is_empty());
    }

    #[test]
    fn middle_sidx_is_owned_and_noop_bytes_are_not_copied() {
        let middle = [mp4_box("moof", b"m"), mp4_box("sidx", b"xx"), mp4_box("mdat", b"data")].concat();
        let stripped = remove_atoms(&middle, &[*b"sidx"]);
        assert!(matches!(stripped, Cow::Owned(_)));
        assert_eq!(stripped.as_ref(), [mp4_box("moof", b"m"), mp4_box("mdat", b"data")].concat());

        let plain = Bytes::from([mp4_box("moof", b"m"), mp4_box("mdat", b"data")].concat());
        let ptr = plain.as_ptr();
        let kept = strip_bytes(plain, "video/mp4", false);
        assert_eq!(kept.as_ptr(), ptr);
    }

    #[test]
    fn atoms_handles_non_utf8_fourcc() {
        let data = [mp4_box_bytes(b"\xa9nam", b"title"), mp4_box("moof", b"m")].concat();
        let found: Vec<[u8; 4]> = atoms(&data).into_iter().map(|entry| entry.0).collect();
        assert_eq!(found, [*b"\xa9nam", *b"moof"]);
    }
}
