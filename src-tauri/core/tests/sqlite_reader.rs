//! Checks the hand-written SQLite reader against databases produced by real
//! SQLite (see `scripts/gen_test_fixtures.py`).
//!
//! The WAL cases are the ones that matter most in practice: Cursor keeps
//! `state.vscdb` in WAL mode while it runs, so the newest values live in the
//! `-wal` file and a reader that only looks at the main file silently reports
//! stale numbers.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use codenotch_core::sqlite::{ReadOnlyDb, Value};

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join(name)
}

fn read_all(name: &str) -> HashMap<String, String> {
    let mut db = ReadOnlyDb::open(&fixture(name)).expect("fixture should open");
    db.key_value_table("ItemTable", 10_000)
        .expect("ItemTable should scan")
}

#[test]
fn reads_a_plain_database() {
    let rows = read_all("simple.db");
    assert_eq!(
        rows.get("cursorAuth/stripeMembershipType")
            .map(String::as_str),
        Some("pro")
    );
    assert_eq!(
        rows.get("cursorAuth/cachedEmail").map(String::as_str),
        Some("dev@example.com")
    );
}

#[test]
fn follows_overflow_chains_for_large_payloads() {
    let rows = read_all("simple.db");
    let big = rows.get("bigValue").expect("bigValue row");
    // 9000 bytes at a 512-byte page size spans many overflow pages.
    assert_eq!(big.len(), 9000);
    assert!(big.chars().all(|c| c == 'x'));
}

#[test]
fn walks_multi_level_btrees() {
    // 400+ rows at a 512-byte page size cannot fit in one leaf, so this only
    // passes if interior pages are traversed.
    let rows = read_all("simple.db");
    assert_eq!(rows.len(), 405, "every row should be visited exactly once");
    assert_eq!(rows.get("filler/0000").map(String::as_str), Some("value-0"));
    assert_eq!(
        rows.get("filler/0399").map(String::as_str),
        Some("value-399")
    );
}

#[test]
fn handles_unicode_keys_and_empty_values() {
    let rows = read_all("simple.db");
    assert_eq!(rows.get("unicode/éè").map(String::as_str), Some("café"));
    assert_eq!(rows.get("emptyValue").map(String::as_str), Some(""));
}

#[test]
fn overlays_committed_wal_frames() {
    let rows = read_all("wal.db");
    // This row exists only in the WAL; a main-file-only reader would miss it.
    assert_eq!(
        rows.get("onlyInWal").map(String::as_str),
        Some("from-the-wal")
    );
    // And an overflow payload written into the WAL still reassembles.
    assert_eq!(rows.get("walOverflow").map(|v| v.len()), Some(9000));
}

#[test]
fn wal_values_win_over_stale_checkpointed_pages() {
    let rows = read_all("wal.db");
    assert_eq!(
        rows.get("checkpointed").map(String::as_str),
        Some("updated-in-wal"),
        "the WAL copy of a page must shadow the main file's copy"
    );
}

#[test]
fn ignores_frames_from_an_uncommitted_transaction() {
    let rows = read_all("wal.db");
    assert!(
        !rows.contains_key("uncommitted"),
        "frames after the last commit must not be visible"
    );
}

#[test]
fn reports_that_a_wal_overlay_is_in_use() {
    let db = ReadOnlyDb::open(&fixture("wal.db")).unwrap();
    assert!(db.has_wal_overlay());

    let db = ReadOnlyDb::open(&fixture("simple.db")).unwrap();
    assert!(!db.has_wal_overlay());
}

#[test]
fn reading_leaves_the_files_untouched() {
    // The whole point of this reader: opening a live database must not write to
    // it, not even a journal or -shm sidecar.
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("state.vscdb");
    std::fs::copy(fixture("wal.db"), &db_path).unwrap();
    std::fs::copy(fixture("wal.db-wal"), dir.path().join("state.vscdb-wal")).unwrap();

    let before: Vec<_> = snapshot_dir(dir.path());
    {
        let mut db = ReadOnlyDb::open(&db_path).unwrap();
        let rows = db.key_value_table("ItemTable", 10_000).unwrap();
        assert!(!rows.is_empty());
    }
    let after: Vec<_> = snapshot_dir(dir.path());

    assert_eq!(
        before, after,
        "reading must not add, remove or modify files"
    );
}

fn snapshot_dir(dir: &Path) -> Vec<(String, u64, std::time::SystemTime)> {
    let mut out: Vec<_> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| {
            let e = e.unwrap();
            let m = e.metadata().unwrap();
            (
                e.file_name().to_string_lossy().into_owned(),
                m.len(),
                m.modified().unwrap(),
            )
        })
        .collect();
    out.sort();
    out
}

#[test]
fn early_exit_stops_the_scan() {
    let mut db = ReadOnlyDb::open(&fixture("simple.db")).unwrap();
    let mut seen = 0usize;
    db.scan_table("ItemTable", |_rowid, _cols| {
        seen += 1;
        seen < 3
    })
    .unwrap();
    assert_eq!(seen, 3, "the visitor's `false` must stop the walk");
}

#[test]
fn rows_expose_typed_columns() {
    let mut db = ReadOnlyDb::open(&fixture("simple.db")).unwrap();
    let mut first: Option<(i64, Vec<Value>)> = None;
    db.scan_table("ItemTable", |rowid, cols| {
        first = Some((rowid, cols.to_vec()));
        false
    })
    .unwrap();
    let (rowid, cols) = first.expect("at least one row");
    assert!(rowid >= 1);
    assert_eq!(cols.len(), 2, "ItemTable has (key, value)");
    assert!(matches!(cols[0], Value::Text(_)));
}

#[test]
fn a_missing_table_is_a_clean_error() {
    let mut db = ReadOnlyDb::open(&fixture("simple.db")).unwrap();
    let err = db
        .key_value_table("NoSuchTable", 10)
        .unwrap_err()
        .to_string();
    assert!(err.contains("not found"), "got: {err}");
}

#[test]
fn a_truncated_wal_degrades_to_the_main_file() {
    // Simulates catching the WAL mid-write: the reader should fall back to the
    // checkpointed pages rather than returning garbage or erroring out.
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("state.vscdb");
    std::fs::copy(fixture("wal.db"), &db_path).unwrap();

    let wal = std::fs::read(fixture("wal.db-wal")).unwrap();
    // Keep the header and part of the first frame only.
    std::fs::write(dir.path().join("state.vscdb-wal"), &wal[..40]).unwrap();

    let mut db = ReadOnlyDb::open(&db_path).unwrap();
    let rows = db.key_value_table("ItemTable", 10_000).unwrap();
    assert_eq!(
        rows.get("checkpointed").map(String::as_str),
        Some("in-main-file"),
        "falls back to the last checkpointed value"
    );
    assert!(!rows.contains_key("onlyInWal"));
}

#[test]
fn a_corrupt_wal_header_is_ignored_rather_than_fatal() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("state.vscdb");
    std::fs::copy(fixture("wal.db"), &db_path).unwrap();

    let mut wal = std::fs::read(fixture("wal.db-wal")).unwrap();
    wal[0] = 0xFF; // wreck the magic
    std::fs::write(dir.path().join("state.vscdb-wal"), &wal).unwrap();

    let mut db = ReadOnlyDb::open(&db_path).unwrap();
    assert!(!db.has_wal_overlay());
    assert!(db.key_value_table("ItemTable", 10).is_ok());
}

#[test]
fn a_limit_caps_how_much_is_retained() {
    let mut db = ReadOnlyDb::open(&fixture("simple.db")).unwrap();
    let rows = db.key_value_table("ItemTable", 10).unwrap();
    assert_eq!(rows.len(), 10);
}
