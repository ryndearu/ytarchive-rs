use super::*;
use crate::types::new_format_info;

fn fi(title: &str) -> FormatInfo {
    let mut f = new_format_info();
    f.insert("title".into(), title.into());
    f.insert("id".into(), "vid".into());
    f.insert("channel".into(), "Chan/nel".into());
    f
}

/// Options with `--windows-filenames` on or off.
fn opts(format: &str, windows: bool) -> Options {
    Options { output_format: format.into(), windows_filenames: windows, ..Default::default() }
}

#[test]
fn location_from_template() {
    let o = opts("%(channel)s/%(title)s-%(id)s", true);
    let (dir, name) = final_location(&o, &fi("A: B")).unwrap();
    assert_eq!(name, "A_ B-vid");
    assert!(dir.is_absolute());
    assert!(dir.ends_with("Chan_nel"));
}

#[test]
fn minimal_sanitizing() {
    let o = opts("%(channel)s/%(title)s", false);
    let mut f = fi("Why? A: B.");
    let (dir, name) = final_location(&o, &f).unwrap();
    assert_eq!(name, "Why? A: B.");
    // A separator in a value still can't create a folder.
    assert!(dir.ends_with("Chan_nel"));
    // Reserved names are left alone. (Not checked on Windows, where
    // making "Con" absolute yields the device path `\\.\Con`, which is
    // why --windows-filenames is the default there.)
    if cfg!(not(windows)) {
        f.insert("channel".into(), "Con".into());
        assert!(final_location(&o, &f).unwrap().0.ends_with("Con"));
    }
}

#[test]
fn leading_dash_and_empty_names() {
    let o = opts("%(title)s", true);
    assert_eq!(final_location(&o, &fi("-x")).unwrap().1, "_-x");
    assert!(final_location(&o, &fi("")).is_err());
    assert!(final_location(&o, &fi(".")).is_err());
    assert_eq!(final_location(&o, &fi("aux")).unwrap().1, "_aux");
    assert_eq!(final_location(&o, &fi("Why?")).unwrap().1, "Why_");
}

#[test]
fn folder_names_are_windows_safe() {
    let o = opts("out/%(channel)s/%(title)s", true);
    let mut f = fi("t");
    f.insert("channel".into(), "Con".into());
    let (dir, _) = final_location(&o, &f).unwrap();
    assert!(dir.ends_with("out/_Con"), "{}", dir.display());
    f.insert("channel".into(), "Mr. ".into());
    let (dir, _) = final_location(&o, &f).unwrap();
    assert!(dir.ends_with("out/Mr__"), "{}", dir.display());
    // `..` in the template itself is kept.
    let up = opts("../%(title)s", true);
    let (dir, _) = final_location(&up, &f).unwrap();
    assert_eq!(dir, std::path::absolute("..").unwrap());
}

#[test]
fn title_is_cut_to_fit_windows_max_path() {
    let units = |p: &Path| p.as_os_str().to_string_lossy().encode_utf16().count();
    let deep = "d".repeat(120);
    let o = opts(&format!("{deep}/%(title)s [%(id)s]"), true);
    let (dir, name) = final_location(&o, &fi(&"t".repeat(200))).unwrap();
    assert!(name.ends_with(" [vid]"), "{name}");
    let longest = dir.join(format!("{name}.f399.frag12345678.ts"));
    assert!(units(&longest) < WINDOWS_MAX_PATH, "{}", units(&longest));
    let temp = dir.join("vid__ffffffff").join(format!("{name}.f399.frag12345678.ts"));
    assert!(units(&temp) < WINDOWS_MAX_PATH, "{}", units(&temp));

    // A longer --temporary-dir leaves less room. Nested inside the output
    // folder so it's longer whatever the working directory is.
    let temp_dir = PathBuf::from(format!("{deep}/{}", "t".repeat(30)));
    let o = Options { temp_dir: Some(temp_dir.clone()), ..o };
    let (_, short) = final_location(&o, &fi(&"t".repeat(200))).unwrap();
    assert!(short.len() < name.len());
    let temp = std::path::absolute(&temp_dir).unwrap().join("vid__ffffffff").join(format!("{short}.f399.frag12345678.ts"));
    assert!(units(&temp) < WINDOWS_MAX_PATH, "{}", units(&temp));

    // Without --windows-filenames, only the 255-byte name limit applies.
    let o = opts(&format!("{deep}/%(title)s [%(id)s]"), false);
    assert_eq!(final_location(&o, &fi(&"t".repeat(200))).unwrap().1.len(), 206);
}

#[test]
fn temp_paths() {
    let p = OutputPlan { final_dir: "/f".into(), file_name: "n".into(), temp_dir: "/t/vid__ab12".into() };
    assert_eq!(p.temp(".f140.ts"), PathBuf::from("/t/vid__ab12/n.f140.ts"));
    assert_eq!(p.final_path(".jpg"), PathBuf::from("/f/n.jpg"));
    assert!(p.owns_temp_dir("vid"));
    assert!(!p.owns_temp_dir("other"));
    let home = OutputPlan { temp_dir: "/home/user".into(), ..p.clone() };
    assert!(!home.owns_temp_dir("vid"));
    let same = OutputPlan { temp_dir: "/f".into(), ..p };
    assert!(!same.owns_temp_dir("vid"));
}

#[test]
fn state_paths_and_loading() {
    let dir = std::env::temp_dir().join(format!("ytarchive-rs-out-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let o = Options { temp_dir: Some(dir.clone()), ..Default::default() };
    let path = state_file(&o, "vid", 140);
    assert_eq!(path, dir.join("vid.f140.state"));
    // serde_json escapes Windows backslashes in the path.
    let json = serde_json::json!({"StartFrag": 5, "Fragments": 2, "Size": 10, "TempDir": dir});
    std::fs::write(&path, json.to_string()).unwrap();
    let (states, prev) = load_states(&o, "vid", &[140, 299]);
    assert_eq!(states[0].start_frag, 5);
    assert_eq!(states[0].file, path);
    assert_eq!(states[1].fragments, 0);
    assert_eq!(prev.as_deref(), Some(dir.as_path()));

    let disabled = Options { disable_save_state: true, ..o };
    assert_eq!(load_states(&disabled, "vid", &[140]).0[0].fragments, 0);
    std::fs::remove_dir_all(&dir).unwrap();
}
