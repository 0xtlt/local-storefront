//! `lsf fmt`: formats the Liquid files of a theme as Prettier does with Shopify's plugin.

mod config;

use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use config::{Configs, IgnoreFile};

#[derive(clap::Args)]
pub struct Args {
    /// The files and the directories to format. Default: the theme directory.
    paths: Vec<PathBuf>,

    /// Writes nothing: lists the files that are not formatted, and fails if there are any.
    #[arg(long)]
    check: bool,

    /// Reads a Liquid source from the standard input and prints it formatted.
    #[arg(long, conflicts_with_all = ["check", "paths"])]
    stdin: bool,

    /// With `--stdin`: the file the source comes from, for its configuration and
    /// `.prettierignore`. An ignored file is printed as it is.
    #[arg(long, value_name = "PATH", requires = "stdin")]
    stdin_filepath: Option<PathBuf>,

    /// Sets one of Prettier's options, under the name Prettier gives it: `--option
    /// printWidth=100`. It wins over the configuration files.
    #[arg(long = "option", short = 'o', value_name = "NAME=VALUE")]
    options: Vec<String>,

    /// Does not read `.prettierrc`, `.editorconfig` nor `.prettierignore`.
    #[arg(long)]
    no_config: bool,
}

/// `name=value` → the option and its value: JSON when it reads as JSON, a string otherwise.
fn parse_option(option: &str) -> Result<(&str, serde_json::Value), String> {
    let (name, value) = option.split_once('=').ok_or_else(|| {
        format!("`--option {option}`: expected NAME=VALUE, as in `printWidth=100`")
    })?;
    let value = serde_json::from_str(value)
        .unwrap_or_else(|_| serde_json::Value::String(value.to_string()));
    Ok((name, value))
}

/// The Liquid files under `directory`, in the order of their names, and the directories that
/// could not be read. Links to directories are not followed, as Prettier does not follow them.
fn liquid_files(
    directory: &Path,
    ignore: Option<&IgnoreFile>,
    found: &mut Vec<PathBuf>,
    unreadable: &mut Vec<String>,
) {
    let entries = match fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(error) => {
            unreadable.push(format!("{}: {error}", directory.display()));
            return;
        }
    };
    let mut entries: Vec<(PathBuf, fs::FileType)> = entries
        .flatten()
        .filter_map(|entry| Some((entry.path(), entry.file_type().ok()?)))
        .collect();
    entries.sort_by(|a, b| a.0.cmp(&b.0));
    for (path, kind) in entries {
        let name = path.file_name().unwrap_or_default().to_string_lossy();
        let is_directory = kind.is_dir();
        if kind.is_symlink() && path.is_dir() {
            continue;
        }
        // A file is also left out when a directory above it is, the one walked from included.
        let ignored = ignore.is_some_and(|ignore| {
            if is_directory {
                ignore.ignores(&path, true)
            } else {
                ignore.ignores_file(&path)
            }
        });
        if ignored {
            continue;
        }
        if is_directory {
            if name != "node_modules" && name != ".git" {
                liquid_files(&path, ignore, found, unreadable);
            }
        } else if name.ends_with(".liquid") {
            found.push(path);
        }
    }
}

/// Replaces a file without a moment where it is half written: the new content goes to a file
/// beside it, which then takes its place with its permissions. A link is followed, so that the
/// file it points to is the one replaced.
fn write_atomically(path: &Path, content: &str) -> std::io::Result<()> {
    let target = if fs::symlink_metadata(path)?.file_type().is_symlink() {
        fs::canonicalize(path)?
    } else {
        path.to_path_buf()
    };
    // A read-only file stays as it is, as with a write in place.
    if fs::metadata(&target)?.permissions().readonly() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "the file is read-only",
        ));
    }
    let directory = target.parent().unwrap_or(Path::new("."));
    let name = target.file_name().unwrap_or_default().to_string_lossy();
    let temporary = directory.join(format!(".{name}.lsf-fmt-{}", std::process::id()));
    let written = fs::write(&temporary, content)
        .and_then(|()| fs::set_permissions(&temporary, fs::metadata(&target)?.permissions()))
        .and_then(|()| fs::rename(&temporary, &target));
    if written.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    written
}

struct Formatter {
    configs: Configs,
    overrides: Vec<(String, serde_json::Value)>,
    read_config: bool,
}

impl Formatter {
    fn options_for(&mut self, file: &Path) -> Result<lsf_fmt::Options, String> {
        let mut options = if self.read_config {
            self.configs.options_for(file)?
        } else {
            lsf_fmt::Options::default()
        };
        for (name, value) in &self.overrides {
            if !options.set(name, value)? {
                return Err(format!(
                    "`--option {name}`: not an option of the formatter. See docs/formatting.md for the list"
                ));
            }
        }
        Ok(options)
    }
}

/// How deep the stack of the formatter may go: a file nested thousands of elements deep is
/// formatted rather than ending the process.
const STACK_SIZE: usize = 512 * 1024 * 1024;

pub fn run(theme: &Path, args: Args) -> Result<ExitCode, String> {
    let theme = theme.to_path_buf();
    std::thread::Builder::new()
        .name("lsf fmt".to_string())
        .stack_size(STACK_SIZE)
        .spawn(move || format_files(&theme, args))
        .map_err(|error| format!("cannot start the formatter: {error}"))?
        .join()
        .unwrap_or_else(|_| Err("the formatter stopped on an error of its own".to_string()))
}

fn format_files(theme: &Path, args: Args) -> Result<ExitCode, String> {
    let overrides = args
        .options
        .iter()
        .map(|option| parse_option(option).map(|(name, value)| (name.to_string(), value)))
        .collect::<Result<Vec<_>, _>>()?;
    let mut formatter = Formatter {
        configs: Configs::default(),
        overrides,
        read_config: !args.no_config,
    };
    // Configuration files are looked for from where a file is, whatever the path it was named
    // with.
    let absolute = |path: &Path| std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
    // One `.prettierignore`, as Prettier reads one: the nearest from the theme up. It applies to
    // the files walked and to the files named alike.
    let ignore = formatter
        .read_config
        .then(|| IgnoreFile::find(&absolute(theme)))
        .flatten();
    let ignored = |file: &Path| {
        ignore
            .as_ref()
            .is_some_and(|ignore| ignore.ignores_file(file))
    };

    if args.stdin {
        let mut source = String::new();
        std::io::stdin()
            .read_to_string(&mut source)
            .map_err(|error| format!("cannot read the standard input: {error}"))?;
        let file = absolute(
            &args
                .stdin_filepath
                .clone()
                .unwrap_or_else(|| theme.join("stdin.liquid")),
        );
        if ignored(&file) {
            print!("{source}");
            return Ok(ExitCode::SUCCESS);
        }
        let options = formatter.options_for(&file)?;
        print!("{}", lsf_fmt::format(&source, &options));
        return Ok(ExitCode::SUCCESS);
    }

    let roots = if args.paths.is_empty() {
        vec![theme.to_path_buf()]
    } else {
        args.paths
    };
    let mut files: Vec<(PathBuf, PathBuf)> = Vec::new();
    let mut unreadable = Vec::new();
    for root in &roots {
        // A path is read from the current directory, else from the theme's.
        let path = if root.exists() || root.is_absolute() {
            root.clone()
        } else {
            theme.join(root)
        };
        if path.is_dir() {
            let mut found = Vec::new();
            liquid_files(
                &absolute(&path),
                ignore.as_ref(),
                &mut found,
                &mut unreadable,
            );
            // Named as they were asked for: relative to the directory given.
            let base = absolute(&path);
            files.extend(found.into_iter().map(|file| {
                let shown = file
                    .strip_prefix(&base)
                    .map_or_else(|_| file.clone(), |relative| root.join(relative));
                (file, shown)
            }));
        } else if path.is_file() {
            let file = absolute(&path);
            // Named one by one, as a commit hook does: only Liquid files are formatted, and
            // what `.prettierignore` names is left alone, as Prettier leaves it.
            if !root.to_string_lossy().ends_with(".liquid") {
                eprintln!("{}: skipped, not a Liquid file", root.display());
            } else if !ignored(&file) {
                files.push((file, root.clone()));
            }
        } else {
            return Err(format!("{}: no such file or directory", root.display()));
        }
    }

    // A file named twice, or found from two paths, is formatted once.
    let mut seen = std::collections::HashSet::new();
    files.retain(|(file, _)| seen.insert(file.clone()));

    // A file or a directory that cannot be read, written or formatted is reported, and the
    // others are formatted all the same.
    let mut changed = 0;
    let mut failed = unreadable.len();
    for problem in &unreadable {
        eprintln!("error: {problem}");
    }
    for (path, shown) in &files {
        let result = (|| -> Result<bool, String> {
            let source = fs::read_to_string(path).map_err(|error| error.to_string())?;
            let options = formatter.options_for(path)?;
            let formatted = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                lsf_fmt::format(&source, &options)
            }))
            .map_err(|_| "the formatter failed on this file".to_string())?;
            if formatted == source {
                return Ok(false);
            }
            if !args.check {
                write_atomically(path, &formatted).map_err(|error| error.to_string())?;
            }
            Ok(true)
        })();
        match result {
            Ok(true) => {
                changed += 1;
                println!("{}", shown.display());
            }
            Ok(false) => {}
            Err(error) => {
                failed += 1;
                eprintln!("error: {}: {error}", shown.display());
            }
        }
    }

    for path in &formatter.configs.unreadable {
        eprintln!(
            "warning: {} is not read: only `.prettierrc` (JSON or YAML), `.prettierrc.json`, `.prettierrc.yaml`, the `prettier` key of `package.json` and `.editorconfig` are; the files of its folder get the default options",
            path.display()
        );
    }
    let unchanged = files.len() - changed - failed;
    let failures = if failed > 0 {
        format!(", {failed} failed")
    } else {
        String::new()
    };
    if args.check {
        eprintln!("{changed} file(s) to format, {unchanged} already formatted{failures}");
    } else {
        eprintln!("{changed} file(s) formatted, {unchanged} unchanged{failures}");
    }
    Ok(if failed > 0 {
        ExitCode::from(2)
    } else if args.check && changed > 0 {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A directory of its own for one test, removed when the test is done.
    struct Scratch(PathBuf);

    impl Scratch {
        fn new(name: &str) -> Scratch {
            let root = std::env::temp_dir().join(format!("lsf-fmt-{name}-{}", std::process::id()));
            let _ = fs::remove_dir_all(&root);
            fs::create_dir_all(&root).unwrap();
            Scratch(root)
        }

        fn write(&self, path: &str, content: &str) {
            let path = self.0.join(path);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, content).unwrap();
        }

        fn read(&self, path: &str) -> String {
            fs::read_to_string(self.0.join(path)).unwrap()
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn args(check: bool, options: &[&str]) -> Args {
        Args {
            paths: Vec::new(),
            check,
            stdin: false,
            stdin_filepath: None,
            options: options.iter().map(|option| option.to_string()).collect(),
            no_config: false,
        }
    }

    #[test]
    fn formats_the_liquid_files_of_a_theme_in_place() {
        let theme = Scratch::new("in-place");
        theme.write("sections/a.liquid", "{%if a%}<p>{{a|upcase}}</p>{%endif%}");
        theme.write("snippets/b.liquid", "<p>{{ b }}</p>\n");
        theme.write("assets/c.css", "a{}");
        theme.write("node_modules/x/d.liquid", "{{d}}");

        assert_eq!(run(&theme.0, args(true, &[])).unwrap(), ExitCode::FAILURE);
        assert_eq!(
            theme.read("sections/a.liquid"),
            "{%if a%}<p>{{a|upcase}}</p>{%endif%}"
        );

        assert_eq!(run(&theme.0, args(false, &[])).unwrap(), ExitCode::SUCCESS);
        assert_eq!(
            theme.read("sections/a.liquid"),
            "{% if a %}\n  <p>{{ a | upcase }}</p>\n{% endif %}\n"
        );
        assert_eq!(theme.read("assets/c.css"), "a{}");
        assert_eq!(theme.read("node_modules/x/d.liquid"), "{{d}}");
        assert_eq!(run(&theme.0, args(true, &[])).unwrap(), ExitCode::SUCCESS);
    }

    #[test]
    fn follows_prettier_s_configuration_and_ignore_files() {
        let theme = Scratch::new("config");
        theme.write(
            ".prettierrc.json",
            r#"{ "printWidth": 20, "overrides": [{ "files": "snippets/*.liquid", "options": { "printWidth": 120 } }] }"#,
        );
        theme.write(".prettierignore", "vendor/\n");
        let source = "<p>{{ product.title }}</p>\n";
        theme.write("sections/a.liquid", source);
        theme.write("snippets/b.liquid", source);
        theme.write("vendor/c.liquid", "{{c}}");

        assert_eq!(run(&theme.0, args(false, &[])).unwrap(), ExitCode::SUCCESS);
        assert_eq!(
            theme.read("sections/a.liquid"),
            "<p>\n  {{ product.title }}\n</p>\n"
        );
        assert_eq!(theme.read("snippets/b.liquid"), source);
        assert_eq!(theme.read("vendor/c.liquid"), "{{c}}");

        // An `.editorconfig` applies too, under the configuration of Prettier.
        theme.write(
            ".editorconfig",
            "[*.liquid]\nindent_size = 4\nmax_line_length = 10\n",
        );
        theme.write("sections/a.liquid", source);
        assert_eq!(run(&theme.0, args(false, &[])).unwrap(), ExitCode::SUCCESS);
        assert_eq!(
            theme.read("sections/a.liquid"),
            "<p>\n    {{ product.title }}\n</p>\n"
        );
        fs::remove_file(theme.0.join(".editorconfig")).unwrap();

        // An option of the command line wins over the files.
        theme.write("sections/a.liquid", source);
        assert_eq!(
            run(&theme.0, args(false, &["printWidth=120"])).unwrap(),
            ExitCode::SUCCESS
        );
        assert_eq!(theme.read("sections/a.liquid"), source);
    }

    /// The table of options of the formatting guide names every option, with its default.
    #[test]
    fn the_docs_list_the_options_and_their_defaults() {
        let readme = include_str!("../../../../../docs/formatting.md");
        let table = readme
            .split("| Prettier option | Default | What it changes |")
            .nth(1)
            .expect("the table of Prettier options");
        let mut documented = Vec::new();
        for row in table
            .lines()
            .skip(2)
            .take_while(|line| line.starts_with('|'))
        {
            let cells: Vec<&str> = row
                .split('|')
                .map(|cell| cell.trim().trim_matches('`'))
                .collect();
            let (name, default) = (cells[1], cells[2]);
            let (_, value) = parse_option(&format!("{name}={default}")).unwrap();
            let mut options = lsf_fmt::Options::default();
            assert!(
                options.set(name, &value).unwrap(),
                "{name} is not an option"
            );
            assert_eq!(
                options,
                lsf_fmt::Options::default(),
                "`{default}` is not the default of {name}"
            );
            documented.push(name);
        }
        assert_eq!(documented, lsf_fmt::Options::NAMES);
    }

    #[test]
    fn refuses_what_it_does_not_understand() {
        let theme = Scratch::new("errors");
        theme.write("a.liquid", "{{ a }}\n");
        // Refused before anything is read, or reported for each file (exit code 2).
        let refused = |result: Result<ExitCode, String>| {
            result.map_or(true, |code| code == ExitCode::from(2))
        };
        assert!(refused(run(&theme.0, args(false, &["printWidth"]))));
        assert!(refused(run(
            &theme.0,
            args(false, &["experimentalTernaries=true"])
        )));
        assert!(refused(run(&theme.0, args(false, &["semi=1"]))));
        assert!(refused(run(&theme.0, args(false, &["printWidth=wide"]))));
        theme.write(".prettierrc", "{ not json");
        assert!(refused(run(&theme.0, args(false, &[]))));
    }

    fn args_for(paths: &[PathBuf]) -> Args {
        Args {
            paths: paths.to_vec(),
            ..args(false, &[])
        }
    }

    #[test]
    fn files_named_one_by_one_are_formatted_as_prettier_would() {
        let theme = Scratch::new("named");
        theme.write(".prettierignore", "vendor/\n");
        theme.write("sections/a.liquid", "{{a}}");
        theme.write("vendor/b.liquid", "{{b}}");
        theme.write("assets/c.js", "const a = 1;\nconst b = 2;\n");
        let path = |name: &str| theme.0.join(name);
        let code = run(
            &theme.0,
            args_for(&[
                path("sections/a.liquid"),
                path("vendor/b.liquid"),
                path("assets/c.js"),
            ]),
        )
        .unwrap();
        assert_eq!(code, ExitCode::SUCCESS);
        assert_eq!(theme.read("sections/a.liquid"), "{{ a }}\n");
        // Ignored, and not Liquid: left as they are.
        assert_eq!(theme.read("vendor/b.liquid"), "{{b}}");
        assert_eq!(theme.read("assets/c.js"), "const a = 1;\nconst b = 2;\n");
        // A path is also looked for in the theme.
        theme.write("sections/a.liquid", "{{a}}");
        run(&theme.0, args_for(&[PathBuf::from("sections")])).unwrap();
        assert_eq!(theme.read("sections/a.liquid"), "{{ a }}\n");
    }

    #[test]
    fn an_ignored_directory_is_ignored_however_it_is_named() {
        let theme = Scratch::new("ignored-dir");
        theme.write(".prettierignore", "vendor/\n");
        theme.write("vendor/b.liquid", "{{b}}");
        theme.write("vendor/sub/c.liquid", "{{c}}");
        theme.write("a.liquid", "{{a}}");
        let paths = [
            theme.0.join("vendor"),
            theme.0.join("a.liquid"),
            theme.0.join("a.liquid"),
        ];
        assert_eq!(run(&theme.0, args_for(&paths)).unwrap(), ExitCode::SUCCESS);
        assert_eq!(theme.read("vendor/b.liquid"), "{{b}}");
        assert_eq!(theme.read("vendor/sub/c.liquid"), "{{c}}");
        assert_eq!(theme.read("a.liquid"), "{{ a }}\n");
    }

    #[test]
    fn a_file_that_fails_does_not_stop_the_others() {
        let theme = Scratch::new("failing");
        theme.write("a.liquid", "{{a}}");
        fs::write(theme.0.join("b.liquid"), [0xff, 0xfe, 0x00]).unwrap();
        theme.write("c.liquid", "{{c}}");
        assert_eq!(run(&theme.0, args(false, &[])).unwrap(), ExitCode::from(2));
        assert_eq!(theme.read("a.liquid"), "{{ a }}\n");
        assert_eq!(theme.read("c.liquid"), "{{ c }}\n");
        // Deep nesting is formatted rather than ending the process: 8,000 elements overflow
        // the 8 MB of a main thread.
        theme.write("b.liquid", &"<div>".repeat(8_000));
        assert_eq!(run(&theme.0, args(false, &[])).unwrap(), ExitCode::SUCCESS);
    }

    #[test]
    fn options_that_cannot_go_together_are_refused() {
        #[derive(clap::Parser)]
        struct Command {
            #[command(flatten)]
            args: Args,
        }
        use clap::Parser;
        let parses = |line: &[&str]| Command::try_parse_from(line).is_ok();
        assert!(parses(&[
            "fmt",
            "--stdin",
            "--stdin-filepath",
            "snippets/a.liquid"
        ]));
        assert!(!parses(&["fmt", "--stdin", "--check"]));
        assert!(!parses(&["fmt", "--stdin", "sections"]));
        assert!(!parses(&["fmt", "--stdin-filepath", "a.liquid"]));
    }

    #[cfg(unix)]
    #[test]
    fn a_file_is_replaced_whole_with_its_permissions_and_links_are_not_followed() {
        use std::os::unix::fs::{PermissionsExt, symlink};
        let theme = Scratch::new("atomic");
        theme.write("sections/a.liquid", "{{a}}");
        fs::set_permissions(
            theme.0.join("sections/a.liquid"),
            fs::Permissions::from_mode(0o640),
        )
        .unwrap();
        // A link to a folder above would make the walk go round.
        symlink(&theme.0, theme.0.join("sections/loop")).unwrap();
        assert_eq!(run(&theme.0, args(false, &[])).unwrap(), ExitCode::SUCCESS);
        assert_eq!(theme.read("sections/a.liquid"), "{{ a }}\n");
        let mode = fs::metadata(theme.0.join("sections/a.liquid"))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o640);
        // A read-only file is left as it is, and so is a directory that cannot be read; the
        // others are formatted.
        theme.write("sections/b.liquid", "{{b}}");
        fs::set_permissions(
            theme.0.join("sections/b.liquid"),
            fs::Permissions::from_mode(0o444),
        )
        .unwrap();
        theme.write("locked/c.liquid", "{{c}}");
        fs::set_permissions(theme.0.join("locked"), fs::Permissions::from_mode(0o000)).unwrap();
        theme.write("sections/d.liquid", "{{d}}");
        assert_eq!(run(&theme.0, args(false, &[])).unwrap(), ExitCode::from(2));
        fs::set_permissions(theme.0.join("locked"), fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(theme.read("sections/b.liquid"), "{{b}}");
        assert_eq!(theme.read("sections/d.liquid"), "{{ d }}\n");
        fs::set_permissions(
            theme.0.join("sections/b.liquid"),
            fs::Permissions::from_mode(0o644),
        )
        .unwrap();

        let leftovers: Vec<_> = fs::read_dir(theme.0.join("sections"))
            .unwrap()
            .flatten()
            .filter(|entry| entry.file_name().to_string_lossy().contains("lsf-fmt"))
            .collect();
        assert!(leftovers.is_empty());
    }
}
