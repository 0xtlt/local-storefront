//! The paths and routes given on the command line, as Git Bash writes them on Windows.
//!
//! Git Bash (MSYS) names the drives of Windows `/c`, `/d`: `/c/Users/me` is `C:\Users\me`. It
//! turns such an argument into its Windows form before a Windows program sees it, except when
//! `MSYS_NO_PATHCONV` is set (the way to keep a route as typed, below) or when lsf is started by
//! a program that is not Git Bash's (an npm script run by cmd.exe). One that reaches lsf as typed
//! is, for Windows, a folder of the current drive (`C:\c\Users\me`), which does not exist. On
//! Windows, lsf reads a Git Bash path as the folder it names, for every path option
//! ([`read_git_bash_paths`]) and for the options that take a file or JSON ([`given`]).
//!
//! The same conversion turns a route (`lsf render /`) into the folder Git Bash is installed in
//! (`C:/Program Files/Git/`): [`route`] refuses such a route, and says how to keep it as typed.
//!
//! The conversion is a function of the text and of whether lsf runs on Windows, so that its
//! tests run on every system.

use std::any::TypeId;
use std::borrow::Cow;
use std::path::{Path, PathBuf};

use clap::builder::{PathBufValueParser, TypedValueParser};

/// The Windows form of a Git Bash path: `/c/Users/me` → `C:\Users\me`, `/d` → `D:\`. `None` for
/// any other path: a relative one, `/tmp`, `/cd/x`, `//server/share`.
pub fn windows_form(path: &str) -> Option<String> {
    let rest = path.strip_prefix('/')?;
    let mut chars = rest.chars();
    let drive = chars.next().filter(char::is_ascii_alphabetic)?;
    let rest = chars.as_str();
    if !(rest.is_empty() || rest.starts_with(['/', '\\'])) {
        return None;
    }
    let rest = rest.trim_start_matches(['/', '\\']).replace('/', "\\");
    Some(format!("{}:\\{rest}", drive.to_ascii_uppercase()))
}

/// What lsf reads a path given on the command line as: on Windows (`windows`), a Git Bash path
/// as the folder it names; any other path, and every path on another system, as it is.
pub fn native(text: &str, windows: bool) -> Cow<'_, str> {
    match windows.then(|| windows_form(text)).flatten() {
        Some(form) => Cow::Owned(form),
        None => Cow::Borrowed(text),
    }
}

/// [`native`], on the system lsf runs on: for the options that take a file as text
/// (`--session`, `--checkout`, `--input`, `--admin-source`).
pub fn given(text: &str) -> Cow<'_, str> {
    native(text, cfg!(windows))
}

/// Makes every path option of `command` and of its subcommands (`--theme`, `--data`, `--app`,
/// the files of `fmt`, ...) read a Git Bash path as its Windows form when `windows`, from the
/// command line or from its environment variable. The options are found by the type of their
/// values, so that one added later is read so too.
pub fn read_git_bash_paths(command: clap::Command, windows: bool) -> clap::Command {
    if !windows {
        return command;
    }
    command
        .mut_args(|arg| {
            if arg.get_value_parser().type_id() == TypeId::of::<PathBuf>() {
                arg.value_parser(PathBufValueParser::new().map(from_git_bash))
            } else {
                arg
            }
        })
        .mut_subcommands(|subcommand| read_git_bash_paths(subcommand, true))
}

/// A path option's value, on Windows.
fn from_git_bash(path: PathBuf) -> PathBuf {
    match path.to_str().and_then(windows_form) {
        Some(form) => PathBuf::from(form),
        None => path,
    }
}

/// What to say of a path that does not exist when it looks like a Git Bash path, which lsf reads
/// as such only on Windows: the folder it would name there. `None` for any other path.
pub fn git_bash_hint(path: &Path) -> Option<String> {
    let form = windows_form(path.to_str()?)?;
    Some(format!(
        "It looks like a Git Bash path, for {form} on Windows"
    ))
}

/// A route given on the command line (`render <path>`, `profile <path>`). Refused when it is a
/// Windows path (`C:/Program Files/Git/`), which is what Git Bash makes of a `/` it sees: the
/// error says how to keep the route as typed. Leading slashes count as one, so that the `//`
/// that keeps Git Bash away from a route (`//products/shirt`) still names it.
pub fn route(text: &str) -> Result<String, String> {
    let bytes = text.as_bytes();
    if bytes.len() >= 3
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && matches!(bytes[2], b'/' | b'\\')
    {
        return Err(format!(
            "{text} is a Windows path, not a route: Git Bash turned the / of the route into the \
             folder it is installed in. Keep the route as typed with MSYS_NO_PATHCONV=1 \
             (`MSYS_NO_PATHCONV=1 lsf render /products/shirt`), or start it with // \
             (`lsf render //products/shirt`)"
        ));
    }
    let rest = text.trim_start_matches('/');
    Ok(if rest.len() < text.len() {
        format!("/{rest}")
    } else {
        text.to_string()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_git_bash_path_has_a_windows_form() {
        for (path, form) in [
            (
                "/c/Users/me/project/lsf-data",
                r"C:\Users\me\project\lsf-data",
            ),
            ("/d/themes/dawn/", r"D:\themes\dawn\"),
            ("/c", r"C:\"),
            ("/C/", r"C:\"),
            ("/e/a b/c", r"E:\a b\c"),
            (r"/c\Users\me", r"C:\Users\me"),
        ] {
            assert_eq!(windows_form(path).as_deref(), Some(form), "{path}");
        }
        for other in [
            "",
            "/",
            "lsf-data",
            "./c/x",
            "c/x",
            "/tmp/x",
            "/cd/x",
            "/1/x",
            "//server/share",
            r"C:\Users\me",
            "C:/Users/me",
        ] {
            assert_eq!(windows_form(other), None, "{other}");
        }
    }

    #[test]
    fn only_windows_reads_a_git_bash_path_as_its_windows_form() {
        assert_eq!(native("/c/Users/me", true), r"C:\Users\me");
        assert_eq!(native("/c/Users/me", false), "/c/Users/me");
        assert_eq!(native("lsf-data", true), "lsf-data");
        assert_eq!(native(r"C:\Users\me", true), r"C:\Users\me");
    }

    #[test]
    fn a_git_bash_path_that_does_not_exist_gets_its_windows_form() {
        let hint = git_bash_hint(Path::new("/c/Users/me/lsf-data")).unwrap();
        assert!(hint.contains(r"C:\Users\me\lsf-data"), "{hint}");
        assert_eq!(git_bash_hint(Path::new("lsf-data")), None);
        assert_eq!(git_bash_hint(Path::new("/tmp/lsf-data")), None);
    }

    #[test]
    fn a_route_git_bash_made_a_windows_path_of_is_refused() {
        for path in [
            "C:/Program Files/Git/",
            "C:/Program Files/Git/products/shirt",
            r"d:\x",
        ] {
            let error = route(path).unwrap_err();
            assert!(error.contains("MSYS_NO_PATHCONV=1"), "{error}");
            assert!(error.contains(path), "{error}");
        }
        assert_eq!(route("/").unwrap(), "/");
        assert_eq!(
            route("/products/shirt?variant=1").unwrap(),
            "/products/shirt?variant=1"
        );
        // The `//` that keeps Git Bash away from a route.
        assert_eq!(route("//").unwrap(), "/");
        assert_eq!(route("//products/shirt").unwrap(), "/products/shirt");
        assert_eq!(route("products").unwrap(), "products");
    }
}
