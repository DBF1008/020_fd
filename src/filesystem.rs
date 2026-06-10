use std::borrow::Cow;
use std::env;
use std::ffi::{OsStr, OsString};
use std::fs;
use std::io;
#[cfg(any(unix, target_os = "redox"))]
use std::os::unix::fs::FileTypeExt;
use std::path::{Component, Path, PathBuf, Prefix};

use normpath::PathExt;

use crate::dir_entry;

pub fn path_absolute_form(path: &Path) -> io::Result<PathBuf> {
    if path.is_absolute() {
        return Ok(path.to_path_buf());
    }

    let path = path.strip_prefix(".").unwrap_or(path);
    env::current_dir().map(|path_buf| path_buf.join(path))
}

pub fn absolute_path(path: &Path) -> io::Result<PathBuf> {
    let path_buf = path_absolute_form(path)?;

    #[cfg(windows)]
    let path_buf = Path::new(
        path_buf
            .as_path()
            .to_string_lossy()
            .trim_start_matches(r"\\?\"),
    )
    .to_path_buf();

    Ok(path_buf)
}

pub fn is_existing_directory(path: &Path) -> bool {
    // Note: we do not use `.exists()` here, as `.` always exists, even if
    // the CWD has been deleted.
    path.is_dir() && (path.file_name().is_some() || path.normalize().is_ok())
}

pub fn is_empty(entry: &dir_entry::DirEntry) -> bool {
    if let Some(file_type) = entry.file_type() {
        if file_type.is_dir() {
            if let Ok(mut entries) = fs::read_dir(entry.path()) {
                entries.next().is_none()
            } else {
                false
            }
        } else if file_type.is_file() {
            entry.metadata().map(|m| m.len() == 0).unwrap_or(false)
        } else {
            false
        }
    } else {
        false
    }
}

#[cfg(any(unix, target_os = "redox"))]
pub fn is_block_device(ft: fs::FileType) -> bool {
    ft.is_block_device()
}

#[cfg(windows)]
pub fn is_block_device(_: fs::FileType) -> bool {
    false
}

#[cfg(any(unix, target_os = "redox"))]
pub fn is_char_device(ft: fs::FileType) -> bool {
    ft.is_char_device()
}

#[cfg(windows)]
pub fn is_char_device(_: fs::FileType) -> bool {
    false
}

#[cfg(any(unix, target_os = "redox"))]
pub fn is_socket(ft: fs::FileType) -> bool {
    ft.is_socket()
}

#[cfg(windows)]
pub fn is_socket(_: fs::FileType) -> bool {
    false
}

#[cfg(any(unix, target_os = "redox"))]
pub fn is_pipe(ft: fs::FileType) -> bool {
    ft.is_fifo()
}

#[cfg(windows)]
pub fn is_pipe(_: fs::FileType) -> bool {
    false
}

#[cfg(any(unix, target_os = "redox"))]
pub fn osstr_to_bytes(input: &OsStr) -> Cow<'_, [u8]> {
    use std::os::unix::ffi::OsStrExt;
    Cow::Borrowed(input.as_bytes())
}

#[cfg(windows)]
pub fn osstr_to_bytes(input: &OsStr) -> Cow<'_, [u8]> {
    let string = input.to_string_lossy();

    match string {
        Cow::Owned(string) => Cow::Owned(string.into_bytes()),
        Cow::Borrowed(string) => Cow::Borrowed(string.as_bytes()),
    }
}

/// Remove the `./` prefix from a path.
pub fn strip_current_dir(path: &Path) -> &Path {
    path.strip_prefix(".").unwrap_or(path)
}

/// Default value for the path_separator, mainly for MSYS/MSYS2, which set the MSYSTEM
/// environment variable, and we set fd's path separator to '/' rather than Rust's default of '\'.
///
/// Returns Some to use a nonstandard path separator, or None to use rust's default on the target
/// platform.
pub fn default_path_separator() -> Option<String> {
    if cfg!(windows) {
        let msystem = env::var("MSYSTEM").ok()?;
        if !msystem.is_empty() {
            return Some("/".to_owned());
        }
    }
    None
}

/// Replace the path separator in the given path with a custom separator string.
///
/// If `path_separator` is `None`, returns a borrowed `Cow` of the input (zero-cost).
/// Otherwise, iterates through `Path::components()` and rebuilds the path with the
/// custom separator, correctly handling Windows prefixes (UNC, drive letters) and root
/// directories.
pub fn replace_path_separator<'a>(
    path: &'a OsStr,
    path_separator: Option<&str>,
) -> Cow<'a, OsStr> {
    // fast-path — no replacement necessary
    let Some(path_separator) = path_separator else {
        return Cow::Borrowed(path);
    };

    let mut out = OsString::with_capacity(path.len());
    let mut components = Path::new(path).components().peekable();

    while let Some(comp) = components.next() {
        match comp {
            // Absolute paths on Windows are tricky.  A Prefix component is usually a drive
            // letter or UNC path, and is usually followed by RootDir. There are also
            // "verbatim" prefixes beginning with "\\?\" that skip normalization. We choose to
            // ignore verbatim path prefixes here because they're very rare, might be
            // impossible to reach here, and there's no good way to deal with them. If users
            // are doing something advanced involving verbatim windows paths, they can do their
            // own output filtering with a tool like sed.
            Component::Prefix(prefix) => {
                if let Prefix::UNC(server, share) = prefix.kind() {
                    // Prefix::UNC is a parsed version of '\\server\share'
                    out.push(path_separator);
                    out.push(path_separator);
                    out.push(server);
                    out.push(path_separator);
                    out.push(share);
                } else {
                    // All other Windows prefix types are rendered as-is. This results in e.g. "C:" for
                    // drive letters. DeviceNS and Verbatim* prefixes won't have backslashes converted,
                    // but they're not returned by directories fd can search anyway so we don't worry
                    // about them.
                    out.push(comp.as_os_str());
                }
            }

            // Root directory is always replaced with the custom separator.
            Component::RootDir => out.push(path_separator),

            // Everything else is joined normally, with a trailing separator if we're not last
            _ => {
                out.push(comp.as_os_str());
                if components.peek().is_some() {
                    out.push(path_separator);
                }
            }
        }
    }
    Cow::Owned(out)
}

#[cfg(test)]
mod tests {
    use super::{replace_path_separator, strip_current_dir};
    use std::ffi::{OsStr, OsString};
    use std::path::Path;

    #[test]
    fn strip_current_dir_basic() {
        assert_eq!(strip_current_dir(Path::new("./foo")), Path::new("foo"));
        assert_eq!(strip_current_dir(Path::new("foo")), Path::new("foo"));
        assert_eq!(
            strip_current_dir(Path::new("./foo/bar/baz")),
            Path::new("foo/bar/baz")
        );
        assert_eq!(
            strip_current_dir(Path::new("foo/bar/baz")),
            Path::new("foo/bar/baz")
        );
    }

    #[test]
    fn replace_separator_none_is_noop() {
        let path = OsStr::new("foo/bar/baz");
        let result = replace_path_separator(path, None);
        // Should return Borrowed (zero-cost)
        assert!(matches!(result, std::borrow::Cow::Borrowed(_)));
        assert_eq!(&*result, path);
    }

    #[test]
    fn replace_separator_basic() {
        let result = replace_path_separator(OsStr::new("foo/bar/baz"), Some("#"));
        assert_eq!(result, OsString::from("foo#bar#baz"));
    }

    #[test]
    fn replace_separator_single_component() {
        let result = replace_path_separator(OsStr::new("foo"), Some("#"));
        assert_eq!(result, OsString::from("foo"));
    }

    #[test]
    fn replace_separator_empty() {
        let result = replace_path_separator(OsStr::new(""), Some("#"));
        assert_eq!(result, OsString::from(""));
    }

    #[test]
    fn replace_separator_absolute() {
        let result = replace_path_separator(OsStr::new("/foo/bar"), Some("="));
        assert_eq!(result, OsString::from("=foo=bar"));
    }

    #[test]
    fn replace_separator_multi_char() {
        let result = replace_path_separator(OsStr::new("a/b/c"), Some("::"));
        assert_eq!(result, OsString::from("a::b::c"));
    }

    #[test]
    fn replace_separator_root_only() {
        let result = replace_path_separator(OsStr::new("/"), Some("#"));
        assert_eq!(result, OsString::from("#"));
    }
}
