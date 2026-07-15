use stateful_core::{fingerprint_path, fingerprint_reader};
use std::{fs, io::Cursor};

#[test]
fn sha256_fingerprint_distinguishes_missing_empty_and_nonempty_files() {
    let path = std::env::temp_dir().join(format!(
        "stateful-core-fingerprint-{}-{}",
        std::process::id(),
        std::thread::current().name().unwrap_or("test")
    ));
    let _ = fs::remove_file(&path);

    let missing = fingerprint_path(&path).expect("missing files should have a fingerprint");
    assert_eq!(missing.exists, false);
    assert_eq!(missing.byte_len, 0);
    assert_eq!(missing.sha256, None);

    fs::write(&path, []).expect("empty file should be written");
    let empty = fingerprint_path(&path).expect("empty file should fingerprint");
    assert_eq!(empty.exists, true);
    assert_eq!(empty.byte_len, 0);
    assert_eq!(
        empty.sha256.as_deref(),
        Some("e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855")
    );

    fs::write(&path, b"stateful").expect("nonempty file should be written");
    let nonempty = fingerprint_path(&path).expect("nonempty file should fingerprint");
    assert_eq!(nonempty.exists, true);
    assert_eq!(nonempty.byte_len, 8);
    assert_eq!(
        nonempty.sha256.as_deref(),
        Some("58bdfeb61cba4c3ca0a276b86e54c7ebadb30bded1e0de68838af234f8ffbb0a")
    );
    assert_eq!(
        fingerprint_reader(Cursor::new(b"stateful")).expect("reader should fingerprint"),
        nonempty
    );

    fs::remove_file(path).expect("test file should be removed");
}
