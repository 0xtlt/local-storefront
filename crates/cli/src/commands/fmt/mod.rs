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
    #[arg(long)]
    stdin: bool,

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

/// The Liquid files under `directory`, in the order of their names.
fn liquid_files(
    directory: &Path,
    ignore: Option<&IgnoreFile>,
    found: &mut Vec<PathBuf>,
) -> Result<(), String> {
    let mut entries: Vec<PathBuf> = fs::read_dir(directory)
        .map_err(|error| format!("{}: {error}", directory.display()))?
        .flatten()
        .map(|entry| entry.path())
        .collect();
    entries.sort();
    for path in entries {
        let name = path.file_name().unwrap_or_default().to_string_lossy();
        let is_directory = path.is_dir();
        if ignore.is_some_and(|ignore| ignore.ignores(&path, is_directory)) {
            continue;
        }
        if is_directory {
            if name != "node_modules" && name != ".git" {
                liquid_files(&path, ignore, found)?;
            }
        } else if name.ends_with(".liquid") {
            found.push(path);
        }
    }
    Ok(())
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
                    "`--option {name}`: not an option of `lsf fmt`. See the README for the list"
                ));
            }
        }
        Ok(options)
    }
}

pub fn run(theme: &Path, args: Args) -> Result<ExitCode, String> {
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

    if args.stdin {
        let mut source = String::new();
        std::io::stdin()
            .read_to_string(&mut source)
            .map_err(|error| format!("cannot read the standard input: {error}"))?;
        let options = formatter.options_for(&absolute(&theme.join("stdin.liquid")))?;
        print!("{}", lsf_fmt::format(&source, &options));
        return Ok(ExitCode::SUCCESS);
    }

    let roots = if args.paths.is_empty() {
        vec![theme.to_path_buf()]
    } else {
        args.paths
    };
    let mut files = Vec::new();
    for root in &roots {
        if root.is_dir() {
            let ignore = formatter
                .read_config
                .then(|| IgnoreFile::find(&absolute(root)))
                .flatten();
            let mut found = Vec::new();
            liquid_files(&absolute(root), ignore.as_ref(), &mut found)?;
            // Named as they were asked for: relative to the directory given.
            let base = absolute(root);
            files.extend(found.into_iter().map(|path| {
                let shown = path
                    .strip_prefix(&base)
                    .map_or_else(|_| path.clone(), |relative| root.join(relative));
                (path, shown)
            }));
        } else if root.is_file() {
            files.push((absolute(root), root.clone()));
        } else {
            return Err(format!("{}: no such file or directory", root.display()));
        }
    }

    let mut changed = 0;
    for (path, shown) in &files {
        let source =
            fs::read_to_string(path).map_err(|error| format!("{}: {error}", shown.display()))?;
        let options = formatter.options_for(path)?;
        let formatted = lsf_fmt::format(&source, &options);
        if formatted == source {
            continue;
        }
        changed += 1;
        println!("{}", shown.display());
        if !args.check {
            fs::write(path, formatted).map_err(|error| format!("{}: {error}", shown.display()))?;
        }
    }

    for path in &formatter.configs.unreadable {
        eprintln!(
            "warning: {} is not read: only `.prettierrc` (JSON or YAML), `.prettierrc.json`, `.prettierrc.yaml`, the `prettier` key of `package.json` and `.editorconfig` are",
            path.display()
        );
    }
    let unchanged = files.len() - changed;
    if args.check {
        eprintln!("{changed} file(s) to format, {unchanged} already formatted");
        return Ok(if changed == 0 {
            ExitCode::SUCCESS
        } else {
            ExitCode::FAILURE
        });
    }
    eprintln!("{changed} file(s) formatted, {unchanged} unchanged");
    Ok(ExitCode::SUCCESS)
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

    /// The table of options of the README names every option, with its default.
    #[test]
    fn the_readme_lists_the_options_and_their_defaults() {
        let readme = include_str!("../../../../../README.md");
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
        assert!(run(&theme.0, args(false, &["printWidth"])).is_err());
        assert!(run(&theme.0, args(false, &["experimentalTernaries=true"])).is_err());
        assert!(run(&theme.0, args(false, &["semi=1"])).is_err());
        assert!(run(&theme.0, args(false, &["printWidth=wide"])).is_err());
        theme.write(".prettierrc", "{ not json");
        assert!(run(&theme.0, args(false, &[])).is_err());
    }
}
