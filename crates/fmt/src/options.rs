//! How the output is laid out: Prettier's options that apply to Liquid, and those of
//! Shopify's plugin, under the same names and with the same defaults.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HtmlWhitespaceSensitivity {
    /// Whitespace matters around what the default CSS displays inline.
    Css,
    /// Whitespace matters around every element.
    Strict,
    /// Whitespace matters around none.
    Ignore,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CaptureWhitespaceSensitivity {
    /// What a `capture` holds is kept as written.
    Strict,
    /// What a `capture` holds is formatted as the rest.
    Ignore,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EndOfLine {
    Lf,
    Crlf,
    Cr,
    /// The line ending the source uses first.
    Auto,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Options {
    pub print_width: usize,
    pub tab_width: usize,
    pub use_tabs: bool,
    pub end_of_line: EndOfLine,
    /// Quotes of HTML attributes.
    pub single_quote: bool,
    pub bracket_same_line: bool,
    pub single_attribute_per_line: bool,
    pub html_whitespace_sensitivity: HtmlWhitespaceSensitivity,
    /// Quotes of the strings of Liquid.
    pub liquid_single_quote: bool,
    /// Quotes of the strings of embedded CSS and JavaScript.
    pub embedded_single_quote: bool,
    pub single_line_link_tags: bool,
    pub indent_schema: bool,
    pub capture_whitespace_sensitivity: CaptureWhitespaceSensitivity,
    /// Whether a dash separates the name of a `@param` from its description.
    pub liquid_doc_param_dash: bool,
    /// Whether what `<script>`, `<style>`, `{% schema %}`, `{% style %}`, `{% stylesheet %}`
    /// and `{% javascript %}` hold is formatted too.
    pub embedded_language_formatting: bool,
}

impl Default for Options {
    fn default() -> Options {
        Options {
            print_width: 120,
            tab_width: 2,
            use_tabs: false,
            end_of_line: EndOfLine::Lf,
            single_quote: false,
            bracket_same_line: false,
            single_attribute_per_line: false,
            html_whitespace_sensitivity: HtmlWhitespaceSensitivity::Css,
            liquid_single_quote: true,
            embedded_single_quote: true,
            single_line_link_tags: false,
            indent_schema: false,
            capture_whitespace_sensitivity: CaptureWhitespaceSensitivity::Strict,
            liquid_doc_param_dash: true,
            embedded_language_formatting: true,
        }
    }
}

impl Options {
    /// The options [`Options::set`] knows, under the names Prettier gives them.
    pub const NAMES: [&'static str; 15] = [
        "printWidth",
        "tabWidth",
        "useTabs",
        "endOfLine",
        "singleQuote",
        "liquidSingleQuote",
        "embeddedSingleQuote",
        "bracketSameLine",
        "singleAttributePerLine",
        "htmlWhitespaceSensitivity",
        "captureWhitespaceSensitivity",
        "singleLineLinkTags",
        "indentSchema",
        "liquidDocParamDash",
        "embeddedLanguageFormatting",
    ];

    /// Sets the option Prettier calls `name` from its JSON value. `Ok(false)` when the option
    /// is not one that applies to Liquid.
    pub fn set(&mut self, name: &str, value: &serde_json::Value) -> Result<bool, String> {
        let boolean = || {
            value
                .as_bool()
                .ok_or_else(|| format!("`{name}` must be true or false, not {value}"))
        };
        let number = || {
            value
                .as_u64()
                .map(|number| number as usize)
                .ok_or_else(|| format!("`{name}` must be a positive integer, not {value}"))
        };
        let choice = || {
            value
                .as_str()
                .ok_or_else(|| format!("`{name}` must be a string, not {value}"))
        };
        match name {
            "printWidth" => self.print_width = number()?,
            "tabWidth" => self.tab_width = number()?,
            "useTabs" => self.use_tabs = boolean()?,
            "endOfLine" => {
                self.end_of_line = match choice()? {
                    "lf" => EndOfLine::Lf,
                    "crlf" => EndOfLine::Crlf,
                    "cr" => EndOfLine::Cr,
                    "auto" => EndOfLine::Auto,
                    other => {
                        return Err(format!(
                            "`{name}` must be \"lf\", \"crlf\", \"cr\" or \"auto\", not \"{other}\""
                        ));
                    }
                }
            }
            "singleQuote" => self.single_quote = boolean()?,
            "bracketSameLine" => self.bracket_same_line = boolean()?,
            "singleAttributePerLine" => self.single_attribute_per_line = boolean()?,
            "htmlWhitespaceSensitivity" => {
                self.html_whitespace_sensitivity = match choice()? {
                    "css" => HtmlWhitespaceSensitivity::Css,
                    "strict" => HtmlWhitespaceSensitivity::Strict,
                    "ignore" => HtmlWhitespaceSensitivity::Ignore,
                    other => {
                        return Err(format!(
                            "`{name}` must be \"css\", \"strict\" or \"ignore\", not \"{other}\""
                        ));
                    }
                }
            }
            "liquidSingleQuote" => self.liquid_single_quote = boolean()?,
            "embeddedSingleQuote" => self.embedded_single_quote = boolean()?,
            "singleLineLinkTags" => self.single_line_link_tags = boolean()?,
            "indentSchema" => self.indent_schema = boolean()?,
            "captureWhitespaceSensitivity" => {
                self.capture_whitespace_sensitivity = match choice()? {
                    "strict" => CaptureWhitespaceSensitivity::Strict,
                    "ignore" => CaptureWhitespaceSensitivity::Ignore,
                    other => {
                        return Err(format!(
                            "`{name}` must be \"strict\" or \"ignore\", not \"{other}\""
                        ));
                    }
                }
            }
            "liquidDocParamDash" => self.liquid_doc_param_dash = boolean()?,
            "embeddedLanguageFormatting" => {
                self.embedded_language_formatting = match choice()? {
                    "auto" => true,
                    "off" => false,
                    other => {
                        return Err(format!(
                            "`{name}` must be \"auto\" or \"off\", not \"{other}\""
                        ));
                    }
                }
            }
            _ => return Ok(false),
        }
        Ok(true)
    }
}
