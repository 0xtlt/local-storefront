//! What lsf-checkout and the checkout page share of the API of checkout UI extensions, in one
//! file, `assets/api.json`: the releases the checkout runs and what each requires, the resources
//! a metafield declaration can name, and the targets the page renders with their place. The page
//! imports the same file (`api-versions.js`, `extension-host.js`).

use std::sync::OnceLock;

use serde::Deserialize;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct Metadata {
    /// Oldest first.
    pub releases: Vec<Release>,
    pub owner_types: Vec<String>,
    /// In the order of the page.
    pub placements: Vec<Placement>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct Release {
    pub version: String,
    /// `optional` or `required`: whether a metafield declaration needs an `owner_type`.
    pub metafield_owner_type: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Placement {
    pub target: String,
    pub place: String,
}

/// `assets/api.json`, read once.
pub(crate) fn get() -> &'static Metadata {
    static METADATA: OnceLock<Metadata> = OnceLock::new();
    METADATA.get_or_init(|| {
        serde_json::from_str(include_str!("../assets/api.json")).expect("assets/api.json")
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_metadata_is_well_formed() {
        let metadata = get();
        let versions: Vec<&str> = metadata
            .releases
            .iter()
            .map(|r| r.version.as_str())
            .collect();
        assert!(versions.is_sorted(), "oldest first: {versions:?}");
        for release in &metadata.releases {
            assert!(
                ["optional", "required"].contains(&release.metafield_owner_type.as_str()),
                "{}",
                release.version
            );
        }
        let mut targets: Vec<&str> = metadata
            .placements
            .iter()
            .map(|p| p.target.as_str())
            .collect();
        assert!(targets.iter().all(|target| target.starts_with("purchase.")));
        targets.sort();
        targets.dedup();
        assert_eq!(
            targets.len(),
            metadata.placements.len(),
            "one place per target"
        );
        assert!(metadata.owner_types.iter().all(|owner| {
            owner
                .bytes()
                .all(|byte| byte.is_ascii_uppercase() || byte == b'_')
        }));
    }
}
