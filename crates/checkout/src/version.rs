//! The Shopify API releases the checkout supports, and the Function API schemas of each.
//!
//! A Function declares its release with `api_version` in `shopify.extension.toml`. The input
//! query of a Function is checked against the schema of that release, and its output has to fit
//! the result type of the target in that schema (see [`crate::schema`]).
//!
//! The releases are the `releases` of `assets/api.json`, which the page reads too. Adding one
//! takes an entry there; `mise run checkout:schemas` (which reads the same list) writes a schema
//! file only for an API whose schema changed in it, to be listed in [`SCHEMA_FILES`].

use std::fmt;
use std::sync::OnceLock;

use crate::schema::Schema;

/// A Shopify API release: `2026-10`, `2027-01`.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct ApiVersion(&'static str);

/// One schema of a Function API, in `crates/checkout/schemas/<since>/<api>.graphql`: the schema
/// of the API from release `since` on, until a newer file of the same API. Releases that share a
/// schema share its file, and its parsed [`Schema`].
struct SchemaFile {
    since: &'static str,
    api: FunctionApi,
    source: &'static str,
}

macro_rules! schema_file {
    ($since:literal, $api:ident, $name:literal) => {
        SchemaFile {
            since: $since,
            api: FunctionApi::$api,
            source: include_str!(concat!("../schemas/", $since, "/", $name, ".graphql")),
        }
    };
}

const SCHEMA_COUNT: usize = 8;

/// Every file in `crates/checkout/schemas` (a test checks that the list and the folder agree).
static SCHEMA_FILES: [SchemaFile; SCHEMA_COUNT] = [
    schema_file!(
        "2026-10",
        CartCheckoutValidation,
        "cart_checkout_validation"
    ),
    schema_file!("2026-10", CartTransform, "cart_transform"),
    schema_file!("2026-10", DeliveryCustomization, "delivery_customization"),
    schema_file!("2026-10", Discount, "discount"),
    schema_file!("2026-10", OrderDiscount, "order_discount"),
    schema_file!("2026-10", PaymentCustomization, "payment_customization"),
    schema_file!("2026-10", ProductDiscount, "product_discount"),
    schema_file!("2026-10", ShippingDiscount, "shipping_discount"),
];

impl ApiVersion {
    /// Every supported release, oldest first.
    pub fn all() -> impl Iterator<Item = ApiVersion> {
        crate::metadata::get()
            .releases
            .iter()
            .map(|release| ApiVersion(&release.version))
    }

    /// The release written as `api_version` (`2026-10`), or the error to show: it names the
    /// supported ones.
    pub fn parse(text: &str) -> Result<ApiVersion, String> {
        ApiVersion::all()
            .find(|version| version.as_str() == text.trim())
            .ok_or_else(|| {
                format!(
                    "api_version \"{text}\" is not supported (supported: {})",
                    ApiVersion::supported()
                )
            })
    }

    /// The supported releases, for messages: `2026-10, 2027-01`.
    pub fn supported() -> String {
        ApiVersion::all()
            .map(|version| version.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    }

    pub fn as_str(self) -> &'static str {
        self.0
    }

    /// The most recent release.
    pub fn latest() -> ApiVersion {
        ApiVersion::all().last().expect("at least one release")
    }

    /// Whether a checkout UI extension has to give the `owner_type` of each of its metafield
    /// declarations (from 2027-01 on): `metafieldOwnerType` in `assets/api.json`, which
    /// `refusalOf` in `assets/api-versions.js` reads too.
    pub fn requires_metafield_owner_type(self) -> bool {
        crate::metadata::get()
            .releases
            .iter()
            .any(|release| release.version == self.0 && release.metafield_owner_type == "required")
    }

    /// The schema of a Function API in this release, parsed once for every release that shares
    /// its file.
    pub fn schema(self, api: FunctionApi) -> &'static Schema {
        static PARSED: [OnceLock<Schema>; SCHEMA_COUNT] = [const { OnceLock::new() }; SCHEMA_COUNT];
        let index = self.schema_file(api);
        PARSED[index].get_or_init(|| {
            let file = &SCHEMA_FILES[index];
            Schema::parse(file.source).unwrap_or_else(|error| {
                panic!("the {api} schema of {} is invalid: {error}", file.since)
            })
        })
    }

    /// The text of the schema of a Function API in this release: what the typegen of a
    /// JavaScript Function reads when the Function has no `schema.graphql`.
    pub fn schema_source(self, api: FunctionApi) -> &'static str {
        SCHEMA_FILES[self.schema_file(api)].source
    }

    /// The index in [`SCHEMA_FILES`] of the schema of `api` in this release: the file of the
    /// newest release at or before it.
    fn schema_file(self, api: FunctionApi) -> usize {
        schema_file_of(&SCHEMA_FILES, self.0, api)
            .unwrap_or_else(|| panic!("no {api} schema for {}", self.0))
    }
}

/// The index in `files` of the schema of `api` in `release`: the file of the newest release at or
/// before it.
fn schema_file_of(files: &[SchemaFile], release: &str, api: FunctionApi) -> Option<usize> {
    files
        .iter()
        .enumerate()
        .filter(|(_, file)| file.api == api && file.since <= release)
        .max_by_key(|(_, file)| file.since)
        .map(|(index, _)| index)
}

impl fmt::Display for ApiVersion {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.0)
    }
}

impl fmt::Debug for ApiVersion {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "ApiVersion({})", self.0)
    }
}

/// A Function API: the schema its input query and its output follow.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum FunctionApi {
    CartCheckoutValidation,
    CartTransform,
    DeliveryCustomization,
    Discount,
    /// Deprecated.
    OrderDiscount,
    PaymentCustomization,
    /// Deprecated.
    ProductDiscount,
    /// Deprecated.
    ShippingDiscount,
}

impl FunctionApi {
    pub const ALL: [FunctionApi; 8] = [
        FunctionApi::CartCheckoutValidation,
        FunctionApi::CartTransform,
        FunctionApi::DeliveryCustomization,
        FunctionApi::Discount,
        FunctionApi::OrderDiscount,
        FunctionApi::PaymentCustomization,
        FunctionApi::ProductDiscount,
        FunctionApi::ShippingDiscount,
    ];

    /// The name of its schema files in `crates/checkout/schemas/<release>`.
    pub fn name(self) -> &'static str {
        match self {
            FunctionApi::CartCheckoutValidation => "cart_checkout_validation",
            FunctionApi::CartTransform => "cart_transform",
            FunctionApi::DeliveryCustomization => "delivery_customization",
            FunctionApi::Discount => "discount",
            FunctionApi::OrderDiscount => "order_discount",
            FunctionApi::PaymentCustomization => "payment_customization",
            FunctionApi::ProductDiscount => "product_discount",
            FunctionApi::ShippingDiscount => "shipping_discount",
        }
    }
}

impl fmt::Display for FunctionApi {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.name())
    }
}

/// What a target is in the schemas: the Function API it belongs to and the type of its output.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TargetSpec {
    pub api: FunctionApi,
    /// The input object the output of the Function has to fit.
    pub result: &'static str,
    /// A `purchase.*` target of the first generation of the APIs: its result is the
    /// `FunctionRunResult` of the schema, which the new operations are not part of.
    pub legacy: bool,
}

/// The targets the checkout runs. `None` for the others (fulfillment constraints, order
/// routing, pickup generators and their `.fetch` targets).
pub fn target_spec(target: &str) -> Option<TargetSpec> {
    use FunctionApi::*;
    let (api, result, legacy) = match target {
        "cart.transform.run" => (CartTransform, "CartTransformRunResult", false),
        "purchase.cart-transform.run" => (CartTransform, "FunctionRunResult", true),
        "cart.lines.discounts.generate.run" => {
            (Discount, "CartLinesDiscountsGenerateRunResult", false)
        }
        "cart.delivery-options.discounts.generate.run" => (
            Discount,
            "CartDeliveryOptionsDiscountsGenerateRunResult",
            false,
        ),
        "purchase.product-discount.run" => (ProductDiscount, "FunctionRunResult", true),
        "purchase.order-discount.run" => (OrderDiscount, "FunctionRunResult", true),
        "purchase.shipping-discount.run" => (ShippingDiscount, "FunctionRunResult", true),
        "cart.delivery-options.transform.run" => (
            DeliveryCustomization,
            "CartDeliveryOptionsTransformRunResult",
            false,
        ),
        "purchase.delivery-customization.run" => (DeliveryCustomization, "FunctionRunResult", true),
        "cart.payment-methods.transform.run" => (
            PaymentCustomization,
            "CartPaymentMethodsTransformRunResult",
            false,
        ),
        "purchase.payment-customization.run" => (PaymentCustomization, "FunctionRunResult", true),
        "cart.validations.generate.run" => (
            CartCheckoutValidation,
            "CartValidationsGenerateRunResult",
            false,
        ),
        "purchase.validation.run" => (CartCheckoutValidation, "FunctionRunResult", true),
        // The targets that ask for an HTTP request: their result is the request.
        "cart.lines.discounts.generate.fetch" => {
            (Discount, "CartLinesDiscountsGenerateFetchResult", false)
        }
        "cart.delivery-options.discounts.generate.fetch" => (
            Discount,
            "CartDeliveryOptionsDiscountsGenerateFetchResult",
            false,
        ),
        "cart.validations.generate.fetch" => (
            CartCheckoutValidation,
            "CartValidationsGenerateFetchResult",
            false,
        ),
        _ => return None,
    };
    Some(TargetSpec {
        api,
        result,
        legacy,
    })
}

/// The target that replaces a legacy `purchase.*` one.
pub fn successor(legacy_target: &str) -> Option<&'static str> {
    Some(match legacy_target {
        "purchase.cart-transform.run" => "cart.transform.run",
        "purchase.product-discount.run" | "purchase.order-discount.run" => {
            "cart.lines.discounts.generate.run"
        }
        "purchase.shipping-discount.run" => "cart.delivery-options.discounts.generate.run",
        "purchase.delivery-customization.run" => "cart.delivery-options.transform.run",
        "purchase.payment-customization.run" => "cart.payment-methods.transform.run",
        "purchase.validation.run" => "cart.validations.generate.run",
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_supported_versions_are_the_two_releases() {
        assert_eq!(ApiVersion::supported(), "2026-10, 2027-01");
        assert_eq!(ApiVersion::latest().as_str(), "2027-01");
        assert_eq!(ApiVersion::parse("2026-10").unwrap().as_str(), "2026-10");
        assert_eq!(
            ApiVersion::parse(" 2027-01 ").unwrap().to_string(),
            "2027-01"
        );
        assert_eq!(
            ApiVersion::parse("2025-10").unwrap_err(),
            "api_version \"2025-10\" is not supported (supported: 2026-10, 2027-01)"
        );
        for other in ["", "unstable", "latest", "2026-1", "2026-10-01", "2027-04"] {
            assert!(ApiVersion::parse(other).is_err(), "{other}");
        }
    }

    #[test]
    fn the_schema_files_are_the_ones_in_the_folder_and_name_the_releases_that_use_them() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("schemas");
        let mut on_disk = Vec::new();
        for release in std::fs::read_dir(&root).unwrap() {
            let release = release.unwrap();
            for file in std::fs::read_dir(release.path()).unwrap() {
                on_disk.push(format!(
                    "{}/{}",
                    release.file_name().to_string_lossy(),
                    file.unwrap().file_name().to_string_lossy()
                ));
            }
        }
        on_disk.sort();
        let mut listed: Vec<String> = SCHEMA_FILES
            .iter()
            .map(|file| format!("{}/{}.graphql", file.since, file.api.name()))
            .collect();
        listed.sort();
        assert_eq!(listed, on_disk);

        // Each release has a schema of each API, and the header of each file lists the releases
        // that use it, as `mise run checkout:schemas` wrote it.
        for (index, file) in SCHEMA_FILES.iter().enumerate() {
            let users: Vec<&str> = ApiVersion::all()
                .filter(|version| version.schema_file(file.api) == index)
                .map(ApiVersion::as_str)
                .collect();
            let header = file.source.lines().next().unwrap();
            assert!(
                header.ends_with(&format!(", API versions {}.", users.join(", "))),
                "{header}"
            );
        }
    }

    #[test]
    fn releases_that_share_a_schema_parse_it_once() {
        let [older, newer] = ["2026-10", "2027-01"].map(|id| ApiVersion::parse(id).unwrap());
        for api in FunctionApi::ALL {
            assert!(std::ptr::eq(older.schema(api), newer.schema(api)), "{api}");
        }
    }

    #[test]
    fn a_release_whose_schema_changed_has_its_own_file() {
        let file = |since, api| SchemaFile {
            since,
            api,
            source: "",
        };
        let files = [
            file("2026-10", FunctionApi::Discount),
            file("2026-10", FunctionApi::CartTransform),
            file("2027-04", FunctionApi::Discount),
        ];
        let of = |release, api| schema_file_of(&files, release, api);
        assert_eq!(of("2026-10", FunctionApi::Discount), Some(0));
        assert_eq!(of("2027-01", FunctionApi::Discount), Some(0));
        assert_eq!(of("2027-04", FunctionApi::Discount), Some(2));
        assert_eq!(of("2027-07", FunctionApi::Discount), Some(2));
        assert_eq!(of("2027-07", FunctionApi::CartTransform), Some(1));
        assert_eq!(of("2026-07", FunctionApi::Discount), None);
        assert_eq!(of("2027-01", FunctionApi::PaymentCustomization), None);
    }

    #[test]
    fn owner_type_is_required_from_2027_01() {
        assert!(
            !ApiVersion::parse("2026-10")
                .unwrap()
                .requires_metafield_owner_type()
        );
        assert!(
            ApiVersion::parse("2027-01")
                .unwrap()
                .requires_metafield_owner_type()
        );
    }

    #[test]
    fn a_legacy_target_has_a_successor() {
        for target in ["purchase.validation.run", "purchase.product-discount.run"] {
            assert!(target_spec(target).unwrap().legacy);
            assert!(successor(target).is_some());
        }
        assert_eq!(successor("cart.validations.generate.run"), None);
    }
}
