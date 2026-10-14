//! How the output is laid out: Prettier's options that apply to Liquid and to the JavaScript it
//! holds, and those of Shopify's plugin, under the same names and with the same defaults.

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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrailingComma {
    All,
    Es5,
    None,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QuoteProps {
    AsNeeded,
    Consistent,
    Preserve,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ObjectWrap {
    /// An object stays broken when its first property is on a line of its own.
    Preserve,
    /// An object goes on one line when it fits.
    Collapse,
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
    // The options of Prettier's printer of JavaScript, for what `<script>` and
    // `{% javascript %}` hold.
    pub semi: bool,
    pub jsx_single_quote: bool,
    pub quote_props: QuoteProps,
    pub trailing_comma: TrailingComma,
    pub bracket_spacing: bool,
    pub object_wrap: ObjectWrap,
    /// `arrowParens`: `true` for "always", `false` for "avoid".
    pub arrow_parens: bool,
    /// `experimentalOperatorPosition`: `true` for "start", `false` for "end".
    pub operator_at_start: bool,
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
            semi: true,
            jsx_single_quote: false,
            quote_props: QuoteProps::AsNeeded,
            trailing_comma: TrailingComma::All,
            bracket_spacing: true,
            object_wrap: ObjectWrap::Preserve,
            arrow_parens: true,
            operator_at_start: false,
        }
    }
}

impl Options {
    /// The options [`Options::set`] knows, under the names Prettier gives them.
    pub const NAMES: [&'static str; 23] = [
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
        "semi",
        "jsxSingleQuote",
        "quoteProps",
        "trailingComma",
        "bracketSpacing",
        "objectWrap",
        "arrowParens",
        "experimentalOperatorPosition",
    ];

    /// Sets the option Prettier calls `name` from its JSON value. `Ok(false)` when the option
    /// is not one of these.
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
            "semi" => self.semi = boolean()?,
            "jsxSingleQuote" => self.jsx_single_quote = boolean()?,
            "quoteProps" => {
                self.quote_props = match choice()? {
                    "as-needed" => QuoteProps::AsNeeded,
                    "consistent" => QuoteProps::Consistent,
                    "preserve" => QuoteProps::Preserve,
                    other => {
                        return Err(format!(
                            "`{name}` must be \"as-needed\", \"consistent\" or \"preserve\", not \"{other}\""
                        ));
                    }
                }
            }
            "trailingComma" => {
                self.trailing_comma = match choice()? {
                    "all" => TrailingComma::All,
                    "es5" => TrailingComma::Es5,
                    "none" => TrailingComma::None,
                    other => {
                        return Err(format!(
                            "`{name}` must be \"all\", \"es5\" or \"none\", not \"{other}\""
                        ));
                    }
                }
            }
            "bracketSpacing" => self.bracket_spacing = boolean()?,
            "objectWrap" => {
                self.object_wrap = match choice()? {
                    "preserve" => ObjectWrap::Preserve,
                    "collapse" => ObjectWrap::Collapse,
                    other => {
                        return Err(format!(
                            "`{name}` must be \"preserve\" or \"collapse\", not \"{other}\""
                        ));
                    }
                }
            }
            "arrowParens" => {
                self.arrow_parens = match choice()? {
                    "always" => true,
                    "avoid" => false,
                    other => {
                        return Err(format!(
                            "`{name}` must be \"always\" or \"avoid\", not \"{other}\""
                        ));
                    }
                }
            }
            "experimentalOperatorPosition" => {
                self.operator_at_start = match choice()? {
                    "start" => true,
                    "end" => false,
                    other => {
                        return Err(format!(
                            "`{name}` must be \"start\" or \"end\", not \"{other}\""
                        ));
                    }
                }
            }
            _ => return Ok(false),
        }
        Ok(true)
    }
}
