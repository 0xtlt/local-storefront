//! Shopify's feature previews: changes a development store can turn on before they ship.
//!
//! By default `lsf` renders what a live store renders. A preview adds what it would add on a
//! development store that turned it on, and nothing else: without it, a theme that uses it gets
//! the error a live store gives.

use crate::theme::Theme;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Preview {
    /// "Liquid July '26 changes": the `{% block %}` and `{% partial %}` tags, and
    /// `@shopify/partial-rendering`.
    LiquidJuly26,
}

impl Preview {
    pub const ALL: [Preview; 1] = [Preview::LiquidJuly26];

    /// The name given to `--preview`.
    pub fn name(self) -> &'static str {
        match self {
            Preview::LiquidJuly26 => "liquid-july-26",
        }
    }

    /// The name Shopify gives it in the list of feature previews.
    pub fn title(self) -> &'static str {
        match self {
            Preview::LiquidJuly26 => "Liquid July '26 changes",
        }
    }

    /// What it adds, in a sentence.
    pub fn summary(self) -> &'static str {
        match self {
            Preview::LiquidJuly26 => {
                "the {% block %} and {% partial %} tags, and @shopify/partial-rendering"
            }
        }
    }

    pub fn from_name(name: &str) -> Option<Preview> {
        Preview::ALL
            .into_iter()
            .find(|preview| preview.name() == name.trim())
    }

    /// The Liquid tags only this preview has.
    pub fn tags(self) -> &'static [&'static str] {
        match self {
            Preview::LiquidJuly26 => &["block", "partial"],
        }
    }

    /// The preview a tag belongs to, if it belongs to one.
    pub fn of_tag(tag: &str) -> Option<Preview> {
        Preview::ALL
            .into_iter()
            .find(|preview| preview.tags().contains(&tag))
    }

    /// The previews whose tags a Liquid file of the theme uses: what to turn on for the theme
    /// to render as it was written. Read from the closing tags (`endblock`, `endpartial`),
    /// which every use has and which are rarely written otherwise.
    pub fn used_by(theme: &Theme) -> Vec<Preview> {
        let files = theme.files();
        let sources: Vec<std::sync::Arc<str>> =
            ["layout", "templates", "sections", "blocks", "snippets"]
                .iter()
                .flat_map(|directory| {
                    files
                        .list(directory)
                        .iter()
                        .filter(|name| name.ends_with(".liquid"))
                        .filter_map(|name| files.read(&format!("{directory}/{name}")))
                        .collect::<Vec<_>>()
                })
                .collect();
        Preview::ALL
            .into_iter()
            .filter(|preview| {
                preview.tags().iter().any(|tag| {
                    let closing = format!("end{tag}");
                    sources.iter().any(|source| source.contains(&closing))
                })
            })
            .collect()
    }
}
