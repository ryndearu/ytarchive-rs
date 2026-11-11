//! Top-level MP4 box ("atom") scanning and removal. ffmpeg rejects
//! concatenated fragments that repeat `sidx` (and, since 6.1, `ftyp`), so
//! those are stripped before writing.

/// Offset and length of each top-level box, in file order.
pub fn atoms(data: &[u8]) -> Vec<(&str, usize, usize)> {
    let mut out = Vec::new();
    let mut ofs = 0usize;
    while let Some((size, name)) = data.get(ofs..ofs + 8).and_then(|header| header.split_first_chunk::<4>()) {
        let len = u32::from_be_bytes(*size) as usize;
        // len 0 ("to end of file") and len 1 (64-bit size) aren't used in
        // these fragments; stop rather than loop forever as Go could.
        if len < 8 || ofs + len > data.len() {
            break;
        }
        let Ok(name) = std::str::from_utf8(name) else {
            break;
        };
        out.push((name, ofs, len));
        ofs += len;
    }
    out
}

/// Returns `data` without the named top-level boxes.
pub fn remove_atoms(data: &[u8], names: &[&str]) -> Vec<u8> {
    let found = atoms(data);
    let mut out = Vec::with_capacity(data.len());
    let mut pos = 0;
    for (name, ofs, len) in found {
        if names.contains(&name) {
            out.extend_from_slice(data.get(pos..ofs).unwrap_or_default());
            pos = ofs + len;
        }
    }
    out.extend_from_slice(data.get(pos..).unwrap_or_default());
    out
}

/// Boxes to strip from a fragment: `sidx` always, `ftyp` except on the
/// first fragment written.
pub fn strip_for_concat(data: &[u8], mime_type: &str, first: bool) -> Option<Vec<u8>> {
    // A blank MIME type is assumed to be MP4.
    if !(mime_type.ends_with("/mp4") || mime_type.is_empty()) {
        return None;
    }
    let names: &[&str] = if first { &["sidx"] } else { &["sidx", "ftyp"] };
    Some(remove_atoms(data, names))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mp4_box(name: &str, payload: &[u8]) -> Vec<u8> {
        let mut bytes = ((payload.len() + 8) as u32).to_be_bytes().to_vec();
        bytes.extend_from_slice(name.as_bytes());
        bytes.extend_from_slice(payload);
        bytes
    }

    fn sample() -> Vec<u8> {
        [mp4_box("ftyp", b"iso6"), mp4_box("sidx", b"xx"), mp4_box("moof", b"m"), mp4_box("mdat", b"data")].concat()
    }

    #[test]
    fn removes_boxes() {
        let data = sample();
        assert_eq!(
            strip_for_concat(&data, "video/mp4", false).unwrap(),
            [mp4_box("moof", b"m"), mp4_box("mdat", b"data")].concat()
        );
        assert_eq!(
            strip_for_concat(&data, "", true).unwrap(),
            [mp4_box("ftyp", b"iso6"), mp4_box("moof", b"m"), mp4_box("mdat", b"data")].concat()
        );
        assert!(strip_for_concat(&data, "video/webm", false).is_none());
    }

    #[test]
    fn malformed_input_is_left_alone() {
        let zero_len = [0u8, 0, 0, 0, b's', b'i', b'd', b'x', 1, 2, 3];
        assert_eq!(remove_atoms(&zero_len, &["sidx"]), zero_len);
        let truncated = [0u8, 0, 0, 99, b's', b'i', b'd', b'x'];
        assert_eq!(remove_atoms(&truncated, &["sidx"]), truncated);
        assert!(remove_atoms(&[], &["sidx"]).is_empty());
    }
}
