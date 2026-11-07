//! Shared test helpers. Not every test binary uses every helper.
#![allow(dead_code)]

use std::fs;
use std::path::{Path, PathBuf};

/// Fixture directory, relative to the package root so paths in output stay machine-independent.
pub fn fixture(name: &str) -> PathBuf {
    Path::new("tests/fixtures").join(name)
}

/// Compares `actual` with `tests/golden/<rel>`. Run with `UPDATE_GOLDEN=1` to rewrite.
pub fn assert_golden(rel: &str, actual: &str) {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/golden")
        .join(rel);
    if std::env::var_os("UPDATE_GOLDEN").is_some() {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, actual).unwrap();
        return;
    }
    let expected = fs::read_to_string(&path).unwrap_or_else(|_| {
        panic!(
            "missing golden file {}. Run with UPDATE_GOLDEN=1 to create it.",
            path.display()
        )
    });
    if expected != actual {
        let diff = similar::TextDiff::from_lines(expected.as_str(), actual);
        panic!(
            "{} doesn't match. Run with UPDATE_GOLDEN=1 if the change is intended.\n{}",
            path.display(),
            diff.unified_diff().header("expected", "actual")
        );
    }
}
