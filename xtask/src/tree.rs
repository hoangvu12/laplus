//! Walking a directory, once, for the three things here that need to.

use std::io;
use std::path::Path;

/// Every file under `root`, depth first, handed to `visit`.
///
/// Directories are descended into and never visited themselves — all three
/// callers want files. An error reading any directory stops the walk and is
/// returned, rather than being skipped: a measurement that quietly missed a
/// subtree would report a smaller number and look exactly like a correct one.
pub fn walk(root: &Path, visit: &mut impl FnMut(&Path) -> io::Result<()>) -> io::Result<()> {
    for entry in std::fs::read_dir(root)? {
        let path = entry?.path();
        if path.is_dir() {
            walk(&path, visit)?;
        } else {
            visit(&path)?;
        }
    }

    Ok(())
}
