//! The filters of the segment query language, as shopify.dev's "Segment query language
//! reference" (2026-10) lists its attributes and the Help Center's "Shopify-based customer
//! segment filters" describes each one: its query name, its type, the values an enum takes,
//! whether it can be `NULL`, and the named parameters of a function. `segmentFilters` lists them,
//! `segmentFilterSuggestions` searches them, the parser ([`super::language`]) checks a query
//! against them, and [`super::evaluate`] reads them on a customer.
//!
//! The localized names are the Help Center's headings; those of the email and storefront events,
//! one filter each in the API, are lsf's (`Email opened`).

/// The type of a filter, which says what operators and values a clause takes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Date,
    Float,
    Integer,
    /// One of these values (`'SUBSCRIBED'`); an empty list takes any text.
    Enum(&'static [&'static str]),
    Text,
    /// A list of text, `CONTAINS` (`customer_tags`, case ignored).
    TextList,
    /// A list of codes, `CONTAINS` (`customer_countries`).
    EnumList,
    /// A list of IDs, `CONTAINS` (`companies`).
    IdList,
    Boolean,
    /// `MATCHES (…)` with named parameters.
    Function,
    /// `anniversary('<metafield>') <date condition>`.
    Anniversary,
}

/// The type of a parameter of a function.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ParamKind {
    Id,
    Text,
    Date,
    Number,
    Coordinates,
}

/// A named parameter of a function filter.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ParamDef {
    pub name: &'static str,
    pub kind: ParamKind,
    pub localized_name: &'static str,
    pub description: &'static str,
    /// Whether `IN (…)` takes several values.
    pub multiple: bool,
}

/// A filter.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Filter {
    pub query_name: &'static str,
    pub localized_name: &'static str,
    pub kind: Kind,
    /// Whether `IS NULL` and `IS NOT NULL` apply.
    pub nullable: bool,
    pub params: &'static [ParamDef],
}

impl Filter {
    pub fn param(&self, name: &str) -> Option<&'static ParamDef> {
        self.params.iter().find(|param| param.name == name)
    }

    /// The GraphQL type of the filter in `segmentFilters`: `SegmentDateFilter`, …
    pub fn type_name(&self) -> &'static str {
        match self.kind {
            Kind::Date | Kind::Anniversary => "SegmentDateFilter",
            Kind::Float => "SegmentFloatFilter",
            Kind::Integer => "SegmentIntegerFilter",
            Kind::Enum(_) | Kind::EnumList => "SegmentEnumFilter",
            Kind::Text | Kind::TextList => "SegmentStringFilter",
            Kind::IdList => "SegmentAssociationFilter",
            Kind::Boolean => "SegmentBooleanFilter",
            Kind::Function => "SegmentEventFilter",
        }
    }

    /// Whether a clause takes several values of it (a list).
    pub fn multi_value(&self) -> bool {
        matches!(self.kind, Kind::TextList | Kind::EnumList | Kind::IdList)
    }
}

const fn param(
    name: &'static str,
    kind: ParamKind,
    localized_name: &'static str,
    description: &'static str,
    multiple: bool,
) -> ParamDef {
    ParamDef {
        name,
        kind,
        localized_name,
        description,
        multiple,
    }
}

const fn filter(
    query_name: &'static str,
    localized_name: &'static str,
    kind: Kind,
    nullable: bool,
) -> Filter {
    Filter {
        query_name,
        localized_name,
        kind,
        nullable,
        params: &[],
    }
}

const fn function(
    query_name: &'static str,
    localized_name: &'static str,
    params: &'static [ParamDef],
) -> Filter {
    Filter {
        query_name,
        localized_name,
        kind: Kind::Function,
        nullable: true,
        params,
    }
}

const DATE_PARAM: ParamDef = param(
    "date",
    ParamKind::Date,
    "Date",
    "The date of the event.",
    false,
);

const EMAIL_PARAMS: &[ParamDef] = &[
    param(
        "activity_id",
        ParamKind::Id,
        "Marketing activity",
        "The marketing activity ID to filter on.",
        true,
    ),
    param(
        "count",
        ParamKind::Number,
        "Count",
        "The number of times the email event occurred.",
        false,
    ),
    DATE_PARAM,
];

const STOREFRONT_PARAMS: &[ParamDef] = &[
    param(
        "id",
        ParamKind::Id,
        "ID",
        "The products or collections to filter on.",
        true,
    ),
    param(
        "tag",
        ParamKind::Text,
        "Tag",
        "The product tag to filter on.",
        false,
    ),
    param(
        "count",
        ParamKind::Number,
        "Count",
        "The number of times the product or collection was viewed.",
        false,
    ),
    DATE_PARAM,
];

/// The email subscription states (`CustomerEmailMarketingState`, as the Help Center lists them).
pub const EMAIL_STATES: &[&str] = &[
    "NOT_SUBSCRIBED",
    "SUBSCRIBED",
    "PENDING",
    "INVALID",
    "UNSUBSCRIBED",
    "REDACTED",
];

/// The SMS subscription states, as the Help Center lists them.
pub const SMS_STATES: &[&str] = &[
    "SUBSCRIBED",
    "PENDING",
    "REDACTED",
    "UNSUBSCRIBED",
    "NOT_SUBSCRIBED",
];

/// Every filter, in the order of the reference's table.
pub const FILTERS: &[Filter] = &[
    filter(
        "abandoned_checkout_date",
        "Abandoned checkout date",
        Kind::Date,
        true,
    ),
    filter("anniversary", "Anniversary", Kind::Anniversary, true),
    filter("amount_spent", "Amount spent", Kind::Float, false),
    filter("companies", "Companies", Kind::IdList, true),
    filter(
        "created_by_app_id",
        "Created by app id",
        Kind::Integer,
        false,
    ),
    filter(
        "customer_account_status",
        "Customer account status",
        Kind::Enum(&["DECLINED", "DISABLED", "ENABLED", "INVITED"]),
        false,
    ),
    filter(
        "customer_added_date",
        "Customer added date",
        Kind::Date,
        false,
    ),
    filter("customer_cities", "Cities", Kind::EnumList, true),
    filter(
        "customer_countries",
        "Countries or regions",
        Kind::EnumList,
        true,
    ),
    filter(
        "customer_email_domain",
        "Customer email domain",
        Kind::Text,
        true,
    ),
    filter("customer_language", "Customer language", Kind::Text, true),
    filter(
        "customer_regions",
        "States or provinces",
        Kind::EnumList,
        true,
    ),
    filter("customer_tags", "Customer tags", Kind::TextList, true),
    function(
        "customer_within_distance",
        "Customer within distance",
        &[
            param(
                "coordinates",
                ParamKind::Coordinates,
                "Coordinates",
                "The pin location to search around: (latitude, longitude).",
                false,
            ),
            param(
                "distance_km",
                ParamKind::Number,
                "Distance (km)",
                "The radius to search within, in kilometers.",
                false,
            ),
            param(
                "distance_mi",
                ParamKind::Number,
                "Distance (mi)",
                "The radius to search within, in miles.",
                false,
            ),
        ],
    ),
    filter(
        "email_subscription_status",
        "Email subscription status",
        Kind::Enum(EMAIL_STATES),
        true,
    ),
    filter("first_order_date", "First order date", Kind::Date, true),
    filter("last_order_date", "Last order date", Kind::Date, true),
    filter("number_of_orders", "Number of orders", Kind::Integer, false),
    function(
        "orders_placed",
        "Orders placed",
        &[
            param(
                "app_id",
                ParamKind::Id,
                "App",
                "The app that created the order.",
                true,
            ),
            param(
                "location_id",
                ParamKind::Id,
                "Location",
                "The location the order was placed from.",
                true,
            ),
            param(
                "count",
                ParamKind::Number,
                "Count",
                "The number of orders placed.",
                false,
            ),
            param(
                "amount",
                ParamKind::Number,
                "Amount",
                "The amount spent on an order.",
                false,
            ),
            param(
                "sum_amount",
                ParamKind::Number,
                "Sum amount",
                "The amount spent on all orders.",
                false,
            ),
            DATE_PARAM,
        ],
    ),
    filter(
        "predicted_spend_tier",
        "Predicted spend tier",
        Kind::Enum(&["HIGH", "MEDIUM", "LOW"]),
        true,
    ),
    function(
        "products_purchased",
        "Products purchased",
        &[
            param(
                "id",
                ParamKind::Id,
                "Product",
                "The products the customer purchased.",
                true,
            ),
            param(
                "tag",
                ParamKind::Text,
                "Tag",
                "A tag of the products purchased.",
                false,
            ),
            param(
                "quantity",
                ParamKind::Number,
                "Quantity",
                "The quantity purchased per order.",
                false,
            ),
            param(
                "sum_quantity",
                ParamKind::Number,
                "Sum quantity",
                "The quantity purchased across all orders.",
                false,
            ),
            DATE_PARAM,
        ],
    ),
    filter(
        "product_subscription_status",
        "Product subscription status",
        Kind::Enum(&[
            "SUBSCRIBED",
            "CANCELED",
            "EXPIRED",
            "FAILED",
            "NEVER_SUBSCRIBED",
            "PAUSED",
        ]),
        true,
    ),
    filter(
        "rfm_group",
        "RFM group",
        Kind::Enum(&[
            "DORMANT",
            "AT_RISK",
            "PREVIOUSLY_LOYAL",
            "NEEDS_ATTENTION",
            "ALMOST_LOST",
            "LOYAL",
            "PROMISING",
            "ACTIVE",
            "NEW",
            "CHAMPIONS",
            "PROSPECTS",
        ]),
        true,
    ),
    filter(
        "sms_subscription_status",
        "SMS subscription status",
        Kind::Enum(SMS_STATES),
        true,
    ),
    function("shopify_email.bounced", "Email bounced", EMAIL_PARAMS),
    function("shopify_email.clicked", "Email clicked", EMAIL_PARAMS),
    function("shopify_email.delivered", "Email delivered", EMAIL_PARAMS),
    function(
        "shopify_email.marked_as_spam",
        "Email marked as spam",
        EMAIL_PARAMS,
    ),
    function("shopify_email.opened", "Email opened", EMAIL_PARAMS),
    function(
        "shopify_email.unsubscribed",
        "Email unsubscribed",
        EMAIL_PARAMS,
    ),
    function(
        "store_credit_accounts",
        "Store credit accounts",
        &[
            param(
                "balance",
                ParamKind::Number,
                "Balance",
                "The current balance of the store credit account.",
                false,
            ),
            param(
                "currency",
                ParamKind::Text,
                "Currency",
                "The currency of the store credit account.",
                false,
            ),
            param(
                "next_expiry_date",
                ParamKind::Date,
                "Next expiry date",
                "The date of the soonest expiring unspent store credit.",
                false,
            ),
            param(
                "last_credit_date",
                ParamKind::Date,
                "Last credit date",
                "The date the customer last received store credit.",
                false,
            ),
        ],
    ),
    function(
        "storefront_event.collection_viewed",
        "Collection viewed",
        STOREFRONT_PARAMS,
    ),
    function(
        "storefront_event.product_viewed",
        "Product viewed",
        STOREFRONT_PARAMS,
    ),
];

/// The filter of a query name. `storefront.product_viewed`, the Help Center's spelling, is
/// `storefront_event.product_viewed`, the reference's.
pub fn find(name: &str) -> Option<&'static Filter> {
    let name = match name.strip_prefix("storefront.") {
        Some(event) => return find(&format!("storefront_event.{event}")),
        None => name,
    };
    FILTERS.iter().find(|filter| filter.query_name == name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_attribute_of_the_reference_is_a_filter() {
        // The attributes of shopify.dev's table, in its order.
        let reference = [
            "abandoned_checkout_date",
            "anniversary",
            "amount_spent",
            "companies",
            "created_by_app_id",
            "customer_account_status",
            "customer_added_date",
            "customer_cities",
            "customer_countries",
            "customer_email_domain",
            "customer_language",
            "customer_regions",
            "customer_tags",
            "customer_within_distance",
            "email_subscription_status",
            "first_order_date",
            "last_order_date",
            "number_of_orders",
            "orders_placed",
            "predicted_spend_tier",
            "products_purchased",
            "product_subscription_status",
            "rfm_group",
            "sms_subscription_status",
            "shopify_email.bounced",
            "shopify_email.clicked",
            "shopify_email.delivered",
            "shopify_email.marked_as_spam",
            "shopify_email.opened",
            "shopify_email.unsubscribed",
            "store_credit_accounts",
            "storefront_event.collection_viewed",
            "storefront_event.product_viewed",
        ];
        let names: Vec<&str> = FILTERS.iter().map(|filter| filter.query_name).collect();
        assert_eq!(names, reference);
        assert!(find("storefront.product_viewed").is_some());
    }
}
