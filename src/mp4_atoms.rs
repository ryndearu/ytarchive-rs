//! Top-level MP4 box ("atom") scanning and removal. ffmpeg rejects
//! concatenated fragments that repeat `sidx` (and, since 6.1, `ftyp`), so
//! those are stripped before writing.

/// Offset and length of each top-level box, in file order.
pub fn atoms(data: &[u8]) -> Vec<(&str, usize, usize)> {
    let mut out = Vec::new();
    let mut ofs = 0usize;
    while ofs + 8 <= data.len() {
        let len = u32::from_be_bytes([data[ofs], data[ofs + 1], data[ofs + 2], data[ofs + 3]]) as usize;
        // len 0 ("to end of file") and len 1 (64-bit size) aren't used in
        // these fragments; stop rather than loop forever as Go could.
        if len < 8 || ofs + len > data.len() {
            break;
        }
        let Ok(name) = std::str::from_utf8(&data[ofs + 4..ofs + 8]) else {
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
            out.extend_from_slice(&data[pos..ofs]);
            pos = ofs + len;
        }
    }
    out.extend_from_slice(&data[pos..]);
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

    fn bx(name: &str, payload: &[u8]) -> Vec<u8> {
        let mut v = ((payload.len() + 8) as u32).to_be_bytes().to_vec();
        v.extend_from_slice(name.as_bytes());
        v.extend_from_slice(payload);
        v
    }

    fn sample() -> Vec<u8> {
        [bx("ftyp", b"iso6"), bx("sidx", b"xx"), bx("moof", b"m"), bx("mdat", b"data")].concat()
    }

    #[test]
    fn scans_boxes() {
        let names: Vec<_> = atoms(&sample()).iter().map(|a| a.0.to_string()).collect();
        assert_eq!(names, ["ftyp", "sidx", "moof", "mdat"]);
    }

    #[test]
    fn removes_boxes() {
        let d = sample();
        assert_eq!(
            strip_for_concat(&d, "video/mp4", false).unwrap(),
            [bx("moof", b"m"), bx("mdat", b"data")].concat()
        );
        assert_eq!(
            strip_for_concat(&d, "", true).unwrap(),
            [bx("ftyp", b"iso6"), bx("moof", b"m"), bx("mdat", b"data")].concat()
        );
        assert!(strip_for_concat(&d, "video/webm", false).is_none());
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
