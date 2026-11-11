//! Filename-template values and file metadata maps.

use std::collections::BTreeMap;

/// Values exposed to the `%(key)s` filename template.
pub type FormatInfo = BTreeMap<String, String>;

/// Metadata written to the final file (key -> value, values may be templates
/// before expansion).
pub type MetaInfo = BTreeMap<String, String>;

/// All template keys present with empty values, so templates can be
/// validated before extraction fills them in.
pub fn new_format_info() -> FormatInfo {
    super::consts::FORMAT_INFO_KEYS.iter().map(|key| (key.to_string(), String::new())).collect()
}
