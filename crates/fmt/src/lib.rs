//! Formats Liquid theme files the way [Prettier](https://prettier.io) does with Shopify's
//! plugin (`@shopify/prettier-plugin-liquid`), without Node.
//!
//! The source goes through the same steps as in the plugin, each one a port of its
//! counterpart: Shopify's Liquid + HTML parser ([`parser`]), the plugin's preprocessing
//! ([`preprocess`]) and printer ([`printer`]), then Prettier's layout algorithm ([`doc`]).
//!
//! What is ported is under the MIT license, as this crate is: Prettier
//! (<https://github.com/prettier/prettier>), Shopify's theme tools
//! (<https://github.com/Shopify/theme-tools>), and the parsers Prettier reads CSS with
//! (PostCSS, `postcss-values-parser`, `postcss-selector-parser`, `postcss-media-query-parser`).
//!
//! ```
//! use lsf_fmt::{Options, format};
//!
//! let formatted = format("{%if a%}<p>{{a|upcase}}</p>{%endif%}", &Options::default());
//! assert_eq!(formatted, "{% if a %}\n  <p>{{ a | upcase }}</p>\n{% endif %}\n");
//! ```

pub mod ast;
pub mod doc;
mod embed;
mod liquid_doc;
pub mod markup;
pub mod options;
pub mod parser;
mod preprocess;
mod printer;
pub mod tokenizer;

pub use options::{CaptureWhitespaceSensitivity, EndOfLine, HtmlWhitespaceSensitivity, Options};

/// Formats a Liquid source. A source is never refused: what cannot be parsed is left as it
/// was written.
pub fn format(source: &str, options: &Options) -> String {
    // A byte order mark stays where it is.
    if let Some(rest) = source.strip_prefix('\u{feff}') {
        return format!("\u{feff}{}", format(rest, options));
    }
    let end_of_line = match options.end_of_line {
        EndOfLine::Lf => "\n",
        EndOfLine::Crlf => "\r\n",
        EndOfLine::Cr => "\r",
        // The one the source uses first.
        EndOfLine::Auto => match source.find('\r') {
            Some(index) if source[index + 1..].starts_with('\n') => "\r\n",
            Some(_) => "\r",
            None => "\n",
        },
    };
    // Line endings are normalized first, as Prettier does.
    let normalized;
    let source = if source.contains('\r') {
        normalized = source.replace("\r\n", "\n").replace('\r', "\n");
        normalized.as_str()
    } else {
        source
    };
    // A source that holds nothing gives nothing.
    if ast::is_blank(source) {
        return String::new();
    }
    let mut ast = parser::parse(source);
    preprocess::preprocess(&mut ast, source, options);
    let document = printer::Printer::new(&ast, source, options).print_root();
    let formatted = doc::print(
        document,
        &doc::PrintOptions {
            print_width: options.print_width,
            tab_width: options.tab_width,
            use_tabs: options.use_tabs,
        },
    );
    if end_of_line == "\n" {
        formatted
    } else {
        formatted.replace('\n', end_of_line)
    }
}
