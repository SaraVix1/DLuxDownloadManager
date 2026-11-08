//! Small filesystem helpers.

use std::io::Write;
use std::path::Path;

/// Write `contents` to `path`, truncating it. On Unix the file is created with mode `0600`
/// (and an existing file is tightened to `0600`) because callers use this for secrets.
pub(crate) fn write_private(path: &Path, contents: &[u8]) -> std::io::Result<()> {
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let mut file = opts.open(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
    }
    file.write_all(contents)?;
    file.sync_all()
}
