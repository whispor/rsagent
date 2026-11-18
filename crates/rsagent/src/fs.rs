//! Small filesystem helpers shared by the CLI's writers.

use std::path::Path;

/// Write `contents` to `path` atomically: a temp file beside it, then a rename.
///
/// `rename` replaces the destination in a single step on every supported platform, so a
/// reader never observes a partially written file. The temp file lives in the same
/// directory as `path`, because a rename across filesystems is not atomic (and on some
/// platforms not permitted at all).
///
/// # Errors
///
/// Returns the underlying I/O error when the temp file cannot be written or the rename
/// fails; on a failed rename the temp file is removed first.
pub(crate) fn write_atomic(path: &Path, contents: &str) -> std::io::Result<()> {
    let dir = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let name = path.file_name().unwrap_or_default().to_string_lossy();
    let temp = dir.join(format!(".{name}.{}.tmp", std::process::id()));

    std::fs::write(&temp, contents)?;
    if let Err(error) = std::fs::rename(&temp, path) {
        // A failed rename must not leave the temp file behind.
        let _ = std::fs::remove_file(&temp);
        return Err(error);
    }
    Ok(())
}
