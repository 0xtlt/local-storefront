//! `shopifyqlQuery`: ShopifyQL over the orders of the shop, as a table.
//!
//! What lsf reads, of shopify.dev's ShopifyQL reference (2026-10):
//!
//! - `FROM sales` (the schema of sales and their reversals) and the clauses that shape its
//!   result: `SHOW` (metrics, comma-separated), `WHERE` (conditions on dimensions: `=`, `!=`,
//!   `<`, `<=`, `>`, `>=`, `IN (…)`, `NOT IN`, `AND`, `OR`, `NOT`, parentheses), `GROUP BY`
//!   (dimensions), `TIMESERIES` (a time dimension, every period of the range filled, zeros
//!   included), `SINCE` / `UNTIL` / `DURING`, `ORDER BY … ASC|DESC`, `LIMIT`, and `WITH TOTALS`
//!   (a `<metric>__totals` column, "aggregated across the entire result set", ratios recomputed).
//!   `VISUALIZE` is the chart of the editor: the API answers the table, and lsf ignores it, as
//!   it ignores `COMPARE TO` and the other `WITH` modifiers (said in `extensions.lsf.notes`).
//! - Dates: `today`, `yesterday`, `now`, offsets (`-30d`, `-4w`, `-3m`, `-4q`, `-1y`), days
//!   (`2026-01-01`), `startOfDay(-30d)` and the other `startOf…`/`endOf…` functions, and
//!   `DURING` `today`, `yesterday`, `this_week`, `this_month`, `this_quarter`, `this_year`,
//!   `last_week`, `last_month`, `last_quarter`, `last_year`, `bfcm<year>`. `TIMESERIES` without
//!   bounds takes the reference's default range (`day`: the last 30 days, `month`: 3 months, …);
//!   a query without `TIMESERIES` nor bounds covers every sale.
//! - The metrics of `sales` lsf computes: `gross_sales` (the lines before discounts),
//!   `discounts` and `returns` (negative, as Shopify's reports show them), `net_sales`
//!   (gross + discounts + returns), `shipping_charges`, `taxes`, `total_sales` (net + shipping +
//!   taxes), `orders`, `average_order_value` ("(gross sales - discounts) / orders"),
//!   `quantity_ordered`, `quantity_returned`, `net_items_sold`, `customers`, `new_customers`,
//!   `returning_customers`, `orders_first_time`, `orders_returning`. Another metric of the
//!   schema is a parse error saying lsf does not compute it.
//! - Its dimensions: the time ones (`hour`, `day`, `week` from Monday, `month`, `quarter`,
//!   `year`, `day_of_week`, `month_of_year`), the order's (`order_id`, `order_name`,
//!   `sales_channel`, `currency`), the customer's (`customer_id`, `customer_name`,
//!   `customer_email`, `new_or_returning_customer`), the product's (`product_id`,
//!   `product_title`, `product_type`, `product_vendor`, `product_variant_id`,
//!   `product_variant_title`, `product_variant_sku`), the addresses' (`billing_country`,
//!   `billing_city`, `shipping_country`, `shipping_city`), `discount_code`.
//!
//! A sale is a line of an order at its `processedAt` (its product, its gross and its
//! discounts), and the order's shipping and taxes on a row of its own (no product: grouped by a
//! product dimension, those rows are left out); a refund is a return at its date. A
//! cancellation without a refund reverses nothing *(lsf's simplification: Shopify records a
//! reversal)*. Errors are `parseErrors`, in the form of the reference's example ("Column Not
//! Found: Column 'total_sale' not found"); the other messages are lsf's in that form.

use std::collections::{BTreeMap, HashSet};

use chrono::{DateTime, Datelike, Duration, NaiveDate, NaiveDateTime, TimeZone, Utc};
use chrono_tz::Tz;
use serde_json::{Map, Value, json};

use crate::domains::catalog::cents_of;
use crate::graphql::{FieldContext, FieldError, Output};
use crate::resolve::{Obj, ShopResolver};
use crate::state::{Record, ShopState, value};

/// The metrics of the `sales` schema: name, data type, display name. Those lsf computes are in
/// [`COMPUTED`].
pub const SALES_METRICS: &[(&str, &str, &str)] = &[
    (
        "acceptable_fraud_risk_rate",
        "PERCENT",
        "Acceptable fraud risk rate",
    ),
    ("additional_fees", "MONEY", "Additional fees"),
    (
        "amount_spent_per_customer",
        "MONEY",
        "Amount spent per customer",
    ),
    ("average_order_value", "MONEY", "Average order value"),
    ("bundles_ordered", "INTEGER", "Bundles ordered"),
    ("cost_of_goods_sold", "MONEY", "Cost of goods sold"),
    ("customers", "INTEGER", "Customers"),
    ("discount_reversals", "MONEY", "Discount reversals"),
    ("discounts", "MONEY", "Discounts"),
    ("duties", "MONEY", "Duties"),
    ("gift_card_discounts", "MONEY", "Gift card discounts"),
    ("gift_card_gross_sales", "MONEY", "Gift card gross sales"),
    ("gift_card_net_sales", "MONEY", "Gift card net sales"),
    ("gift_card_taxes", "MONEY", "Gift card taxes"),
    ("gross_margin", "PERCENT", "Gross margin"),
    ("gross_profit", "MONEY", "Gross profit"),
    ("gross_sales", "MONEY", "Gross sales"),
    ("gross_sales_reversals", "MONEY", "Gross sales reversals"),
    ("gross_shipping_charges", "MONEY", "Gross shipping charges"),
    ("high_fraud_risk_rate", "PERCENT", "High fraud risk rate"),
    ("line_item_discounts", "MONEY", "Line item discounts"),
    ("net_items_sold", "INTEGER", "Net items sold"),
    ("net_sales", "MONEY", "Net sales"),
    ("net_sales_reversals", "MONEY", "Net sales reversals"),
    (
        "net_sales_with_cost_recorded",
        "MONEY",
        "Net sales with cost recorded",
    ),
    (
        "net_sales_without_cost_recorded",
        "MONEY",
        "Net sales without cost recorded",
    ),
    ("new_customers", "INTEGER", "New customers"),
    (
        "number_of_orders_per_customer",
        "INTEGER",
        "Number of orders per customer",
    ),
    ("order_level_discounts", "MONEY", "Order level discounts"),
    ("order_sale_agreements", "INTEGER", "Order sale agreements"),
    ("orders", "INTEGER", "Orders"),
    ("orders_first_time", "INTEGER", "Orders first time"),
    ("orders_returning", "INTEGER", "Orders returning"),
    ("quantity_ordered", "INTEGER", "Quantity ordered"),
    (
        "quantity_ordered_per_order",
        "INTEGER",
        "Quantity ordered per order",
    ),
    (
        "rate_of_staff_assisted_sales",
        "PERCENT",
        "Rate of staff assisted sales",
    ),
    ("return_fees", "MONEY", "Return fees"),
    (
        "returning_customer_rate",
        "PERCENT",
        "Returning customer rate",
    ),
    ("returning_customers", "INTEGER", "Returning customers"),
    ("reversed_quantity", "INTEGER", "Reversed quantity"),
    (
        "reversed_quantity_rate",
        "PERCENT",
        "Reversed quantity rate",
    ),
    ("sales_reversals", "MONEY", "Sales reversals"),
    (
        "shipping_charge_discounts",
        "MONEY",
        "Shipping charge discounts",
    ),
    ("shipping_charge_taxes", "MONEY", "Shipping charge taxes"),
    ("shipping_charges", "MONEY", "Shipping charges"),
    ("shipping_reversals", "MONEY", "Shipping reversals"),
    ("tax_reversals", "MONEY", "Tax reversals"),
    ("taxes", "MONEY", "Taxes"),
    (
        "taxes_excluding_gift_cards",
        "MONEY",
        "Taxes excluding gift cards",
    ),
    ("tips", "MONEY", "Tips"),
    ("total_sales", "MONEY", "Total sales"),
    ("total_sales_first_time", "MONEY", "Total sales first time"),
    ("total_sales_returning", "MONEY", "Total sales returning"),
    ("total_sales_reversals", "MONEY", "Total sales reversals"),
    ("total_shipping_charges", "MONEY", "Total shipping charges"),
    ("discounts_returned", "MONEY", "Discounts returned"),
    ("gross_returns", "MONEY", "Gross returns"),
    ("net_returns", "MONEY", "Net returns"),
    ("quantity_returned", "INTEGER", "Quantity returned"),
    (
        "returned_quantity_rate",
        "PERCENT",
        "Returned quantity rate",
    ),
    ("returns", "MONEY", "Returns"),
    ("shipping_returned", "MONEY", "Shipping returned"),
    ("taxes_returned", "MONEY", "Taxes returned"),
    ("total_returns", "MONEY", "Total returns"),
];

/// The metrics lsf computes.
const COMPUTED: &[&str] = &[
    "average_order_value",
    "customers",
    "discounts",
    "gross_sales",
    "net_items_sold",
    "net_sales",
    "new_customers",
    "orders",
    "orders_first_time",
    "orders_returning",
    "quantity_ordered",
    "quantity_returned",
    "returning_customers",
    "returns",
    "shipping_charges",
    "taxes",
    "total_sales",
];

/// The dimensions lsf reads: name, data type, display name.
const DIMENSIONS: &[(&str, &str, &str)] = &[
    ("hour", "HOUR_TIMESTAMP", "Hour"),
    ("day", "DAY_TIMESTAMP", "Day"),
    ("week", "WEEK_TIMESTAMP", "Week"),
    ("month", "MONTH_TIMESTAMP", "Month"),
    ("quarter", "QUARTER_TIMESTAMP", "Quarter"),
    ("year", "YEAR_TIMESTAMP", "Year"),
    ("day_of_week", "DAY_OF_WEEK", "Day of week"),
    ("month_of_year", "MONTH_OF_YEAR", "Month of year"),
    ("order_id", "IDENTITY", "Order ID"),
    ("order_name", "STRING", "Order name"),
    ("sales_channel", "STRING", "Sales channel"),
    ("currency", "STRING", "Currency"),
    ("customer_id", "IDENTITY", "Customer ID"),
    ("customer_name", "STRING", "Customer name"),
    ("customer_email", "STRING", "Customer email"),
    (
        "new_or_returning_customer",
        "STRING",
        "New or returning customer",
    ),
    ("product_id", "IDENTITY", "Product ID"),
    ("product_title", "STRING", "Product title"),
    ("product_type", "STRING", "Product type"),
    ("product_vendor", "STRING", "Product vendor"),
    ("product_variant_id", "IDENTITY", "Product variant ID"),
    ("product_variant_title", "STRING", "Product variant title"),
    ("product_variant_sku", "STRING", "Product variant SKU"),
    ("billing_country", "STRING", "Billing country"),
    ("billing_city", "STRING", "Billing city"),
    ("shipping_country", "STRING", "Shipping country"),
    ("shipping_city", "STRING", "Shipping city"),
    ("discount_code", "STRING", "Discount code"),
];

/// The time dimensions, from the finest.
const TIME: &[&str] = &["hour", "day", "week", "month", "quarter", "year"];

const PRODUCT_DIMENSIONS: &[&str] = &[
    "product_id",
    "product_title",
    "product_type",
    "product_vendor",
    "product_variant_id",
    "product_variant_title",
    "product_variant_sku",
];

// --- Tokens ---

#[derive(Clone, Debug, PartialEq)]
enum Tok {
    Word(String),
    Text(String),
    Number(String),
    Symbol(String),
}

impl Tok {
    fn written(&self) -> String {
        match self {
            Tok::Word(text) | Tok::Number(text) | Tok::Symbol(text) => text.clone(),
            Tok::Text(text) => format!("'{text}'"),
        }
    }

    fn is_keyword(&self, keyword: &str) -> bool {
        matches!(self, Tok::Word(word) if word.eq_ignore_ascii_case(keyword))
    }
}

fn lex(text: &str) -> Result<Vec<Tok>, String> {
    let chars: Vec<char> = text.chars().collect();
    let mut tokens = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c.is_whitespace() {
            i += 1;
            continue;
        }
        if c == '\'' || c == '"' {
            let quote = c;
            let mut value = String::new();
            i += 1;
            while i < chars.len() && chars[i] != quote {
                if chars[i] == '\\' && i + 1 < chars.len() {
                    i += 1;
                }
                value.push(chars[i]);
                i += 1;
            }
            if i >= chars.len() {
                return Err("Syntax Error: a string is not closed".into());
            }
            i += 1;
            tokens.push(Tok::Text(value));
            continue;
        }
        if c.is_ascii_digit()
            || ((c == '-' || c == '+') && chars.get(i + 1).is_some_and(char::is_ascii_digit))
        {
            let start = i;
            i += 1;
            while i < chars.len()
                && (chars[i].is_ascii_alphanumeric() || matches!(chars[i], '.' | '-' | ':' | 'T'))
            {
                i += 1;
            }
            tokens.push(Tok::Number(chars[start..i].iter().collect()));
            continue;
        }
        if c.is_alphabetic() || c == '_' {
            let start = i;
            while i < chars.len() && (chars[i].is_alphanumeric() || matches!(chars[i], '_' | '.')) {
                i += 1;
            }
            tokens.push(Tok::Word(chars[start..i].iter().collect()));
            continue;
        }
        let two: String = chars[i..(i + 2).min(chars.len())].iter().collect();
        if ["!=", "<=", ">=", "<>"].contains(&two.as_str()) {
            tokens.push(Tok::Symbol(if two == "<>" { "!=".into() } else { two }));
            i += 2;
            continue;
        }
        if "=<>(),*".contains(c) {
            tokens.push(Tok::Symbol(c.to_string()));
            i += 1;
            continue;
        }
        return Err(format!("Syntax Error: Unexpected character '{c}'"));
    }
    Ok(tokens)
}

// --- The query ---

/// A `WHERE` condition. `AND` and `OR` hold their operands in a list, and a chain of `NOT` is
/// counted, so that only parentheses nest the tree ([`MAX_DEPTH`] deep at most): a long query
/// is wide, never deep, for the recursion that reads it and the one that drops it.
#[derive(Clone, Debug, PartialEq)]
enum Cond {
    And(Vec<Cond>),
    Or(Vec<Cond>),
    Not(Box<Cond>),
    Compare(String, String, Vec<Tok>),
}

/// How deep parentheses nest in a condition. The reference publishes no bound; lsf's is the
/// search syntax's (`search::syntax::MAX_DEPTH`), as the parser recurses once per `(` on the
/// request's stack, which 400,000 of them overflowed.
pub const MAX_DEPTH: usize = 100;

#[derive(Clone, Debug, Default)]
struct Ql {
    from: String,
    show: Vec<String>,
    condition: Option<Cond>,
    group_by: Vec<String>,
    timeseries: Option<String>,
    since: Option<Vec<Tok>>,
    until: Option<Vec<Tok>>,
    during: Option<String>,
    order_by: Vec<(String, bool)>,
    limit: Option<usize>,
    totals: bool,
    ignored: Vec<String>,
}

/// The clause keywords, which end the clause before them.
const CLAUSES: &[&str] = &[
    "FROM",
    "SHOW",
    "WHERE",
    "GROUP",
    "TIMESERIES",
    "SINCE",
    "UNTIL",
    "DURING",
    "ORDER",
    "LIMIT",
    "COMPARE",
    "VISUALIZE",
    "WITH",
    "HAVING",
];

fn is_clause(token: &Tok) -> bool {
    CLAUSES.iter().any(|clause| token.is_keyword(clause))
}

struct Parser {
    tokens: Vec<Tok>,
    at: usize,
    /// How many parentheses of the condition are open.
    depth: usize,
}

impl Parser {
    fn peek(&self) -> Option<&Tok> {
        self.tokens.get(self.at)
    }

    fn next(&mut self) -> Option<Tok> {
        let token = self.tokens.get(self.at).cloned();
        self.at += 1;
        token
    }

    fn unexpected(token: Option<&Tok>) -> String {
        match token {
            Some(token) => format!("Syntax Error: Unexpected '{}'", token.written()),
            None => "Syntax Error: Unexpected end of query".into(),
        }
    }

    fn word(&mut self) -> Result<String, String> {
        match self.next() {
            Some(Tok::Word(word)) if !is_clause(&Tok::Word(word.clone())) => Ok(word),
            other => Err(Self::unexpected(other.as_ref())),
        }
    }

    /// A comma-separated list of names; `AS alias` is read and dropped.
    fn names(&mut self) -> Result<Vec<String>, String> {
        let mut names = vec![self.word()?];
        loop {
            if self.peek().is_some_and(|token| token.is_keyword("AS")) {
                self.next();
                self.word()?;
            }
            if self.peek() == Some(&Tok::Symbol(",".into())) {
                self.next();
                names.push(self.word()?);
            } else {
                return Ok(names);
            }
        }
    }

    /// The tokens up to the next clause: a date bound.
    fn until_clause(&mut self) -> Vec<Tok> {
        let mut tokens = Vec::new();
        while let Some(token) = self.peek() {
            if is_clause(token) {
                break;
            }
            tokens.push(token.clone());
            self.at += 1;
        }
        tokens
    }

    fn condition(&mut self) -> Result<Cond, String> {
        let mut parts = vec![self.conjunction()?];
        while self.peek().is_some_and(|token| token.is_keyword("OR")) {
            self.next();
            parts.push(self.conjunction()?);
        }
        Ok(if parts.len() == 1 {
            parts.remove(0)
        } else {
            Cond::Or(parts)
        })
    }

    fn conjunction(&mut self) -> Result<Cond, String> {
        let mut parts = vec![self.negation()?];
        while self.peek().is_some_and(|token| token.is_keyword("AND")) {
            self.next();
            parts.push(self.negation()?);
        }
        Ok(if parts.len() == 1 {
            parts.remove(0)
        } else {
            Cond::And(parts)
        })
    }

    /// A comparison or a group, after the `NOT`s before it: counted (`NOT NOT x` is `x`).
    fn negation(&mut self) -> Result<Cond, String> {
        let mut negated = false;
        while self.peek().is_some_and(|token| token.is_keyword("NOT")) {
            self.next();
            negated = !negated;
        }
        let operand = self.operand()?;
        Ok(if negated {
            Cond::Not(Box::new(operand))
        } else {
            operand
        })
    }

    fn operand(&mut self) -> Result<Cond, String> {
        if self.peek() == Some(&Tok::Symbol("(".into())) {
            self.next();
            self.depth += 1;
            if self.depth > MAX_DEPTH {
                return Err(format!(
                    "Syntax Error: parentheses nest more than {MAX_DEPTH} deep"
                ));
            }
            let inner = self.condition()?;
            self.depth -= 1;
            match self.next() {
                Some(Tok::Symbol(close)) if close == ")" => return Ok(inner),
                other => return Err(Self::unexpected(other.as_ref())),
            }
        }
        let field = self.word()?;
        let operator = match self.next() {
            Some(Tok::Symbol(op)) if ["=", "!=", "<", "<=", ">", ">="].contains(&op.as_str()) => op,
            Some(token) if token.is_keyword("IN") => "IN".into(),
            Some(token) if token.is_keyword("NOT") => match self.next() {
                Some(token) if token.is_keyword("IN") => "NOT IN".into(),
                other => return Err(Self::unexpected(other.as_ref())),
            },
            other => return Err(Self::unexpected(other.as_ref())),
        };
        let mut values = Vec::new();
        if operator.ends_with("IN") {
            if self.next() != Some(Tok::Symbol("(".into())) {
                return Err(Self::unexpected(self.tokens.get(self.at - 1)));
            }
            loop {
                match self.next() {
                    Some(value @ (Tok::Text(_) | Tok::Number(_) | Tok::Word(_))) => {
                        values.push(value)
                    }
                    other => return Err(Self::unexpected(other.as_ref())),
                }
                match self.next() {
                    Some(Tok::Symbol(comma)) if comma == "," => continue,
                    Some(Tok::Symbol(close)) if close == ")" => break,
                    other => return Err(Self::unexpected(other.as_ref())),
                }
            }
        } else {
            match self.next() {
                Some(value @ (Tok::Text(_) | Tok::Number(_) | Tok::Word(_))) => values.push(value),
                other => return Err(Self::unexpected(other.as_ref())),
            }
        }
        Ok(Cond::Compare(field, operator, values))
    }
}

fn parse(text: &str) -> Result<Ql, String> {
    let mut parser = Parser {
        tokens: lex(text)?,
        at: 0,
        depth: 0,
    };
    let mut query = Ql::default();
    if !parser.peek().is_some_and(|token| token.is_keyword("FROM")) {
        return Err("Syntax Error: a query starts with FROM".into());
    }
    while let Some(token) = parser.next() {
        let Tok::Word(keyword) = &token else {
            return Err(Parser::unexpected(Some(&token)));
        };
        match keyword.to_ascii_uppercase().as_str() {
            "FROM" => query.from = parser.word()?.to_ascii_lowercase(),
            "SHOW" => query.show = parser.names()?,
            "WHERE" => query.condition = Some(parser.condition()?),
            "GROUP" => {
                if !parser.next().is_some_and(|token| token.is_keyword("BY")) {
                    return Err(Parser::unexpected(parser.tokens.get(parser.at - 1)));
                }
                if parser.peek().is_some_and(|token| token.is_keyword("ONLY")) {
                    query.ignored.push("GROUP BY ONLY TOP".into());
                    parser.next();
                    parser.next();
                    parser.next();
                }
                query.group_by = parser.names()?;
            }
            "TIMESERIES" => query.timeseries = Some(parser.word()?.to_ascii_lowercase()),
            "SINCE" => query.since = Some(parser.until_clause()),
            "UNTIL" => query.until = Some(parser.until_clause()),
            "DURING" => query.during = Some(parser.word()?.to_ascii_lowercase()),
            "ORDER" => {
                if !parser.next().is_some_and(|token| token.is_keyword("BY")) {
                    return Err(Parser::unexpected(parser.tokens.get(parser.at - 1)));
                }
                loop {
                    let column = parser.word()?;
                    let mut descending = false;
                    if parser.peek().is_some_and(|token| token.is_keyword("DESC")) {
                        descending = true;
                        parser.next();
                    } else if parser.peek().is_some_and(|token| token.is_keyword("ASC")) {
                        parser.next();
                    }
                    query.order_by.push((column, descending));
                    if parser.peek() == Some(&Tok::Symbol(",".into())) {
                        parser.next();
                    } else {
                        break;
                    }
                }
            }
            "LIMIT" => match parser.next() {
                Some(Tok::Number(number)) => {
                    query.limit = Some(
                        number
                            .parse()
                            .map_err(|_| format!("Syntax Error: Unexpected '{number}'"))?,
                    );
                }
                other => return Err(Parser::unexpected(other.as_ref())),
            },
            "WITH" => {
                for modifier in parser.until_clause() {
                    if let Tok::Word(word) = &modifier {
                        if word.eq_ignore_ascii_case("TOTALS") {
                            query.totals = true;
                        } else {
                            query
                                .ignored
                                .push(format!("WITH {}", word.to_ascii_uppercase()));
                        }
                    }
                }
            }
            "COMPARE" | "VISUALIZE" | "HAVING" => {
                parser.until_clause();
                query.ignored.push(keyword.to_ascii_uppercase());
            }
            _ => return Err(Parser::unexpected(Some(&token))),
        }
        // `TIMESERIES day WITH TOTALS`: the modifiers follow the clause.
    }
    if query.show.is_empty() {
        return Err("Syntax Error: a query needs SHOW".into());
    }
    if query.during.is_some() && (query.since.is_some() || query.until.is_some()) {
        return Err("Syntax Error: DURING can't be combined with SINCE or UNTIL".into());
    }
    Ok(query)
}

/// Checks a ShopifyQL condition (the `filters` of an analytics target).
pub fn check_condition(text: &str) -> Result<(), String> {
    let mut parser = Parser {
        tokens: lex(text)?,
        at: 0,
        depth: 0,
    };
    let condition = parser.condition()?;
    if let Some(token) = parser.peek() {
        return Err(Parser::unexpected(Some(token)));
    }
    check_fields(&condition)
}

fn check_fields(condition: &Cond) -> Result<(), String> {
    match condition {
        Cond::And(parts) | Cond::Or(parts) => parts.iter().try_for_each(check_fields),
        Cond::Not(inner) => check_fields(inner),
        Cond::Compare(field, _, _) => {
            if dimension(field).is_some() {
                Ok(())
            } else {
                Err(not_found(field))
            }
        }
    }
}

fn not_found(column: &str) -> String {
    format!("Column Not Found: Column '{column}' not found")
}

fn dimension(name: &str) -> Option<&'static (&'static str, &'static str, &'static str)> {
    DIMENSIONS.iter().find(|(known, _, _)| *known == name)
}

fn metric(name: &str) -> Option<&'static (&'static str, &'static str, &'static str)> {
    SALES_METRICS.iter().find(|(known, _, _)| *known == name)
}

// --- Dates ---

struct Clock {
    now: DateTime<Utc>,
    zone: Tz,
}

impl Clock {
    fn local(&self, at: DateTime<Utc>) -> NaiveDateTime {
        at.with_timezone(&self.zone).naive_local()
    }

    fn utc(&self, local: NaiveDateTime) -> DateTime<Utc> {
        self.zone
            .from_local_datetime(&local)
            .earliest()
            .map_or_else(
                || Utc.from_utc_datetime(&local),
                |at| at.with_timezone(&Utc),
            )
    }

    fn today(&self) -> NaiveDate {
        self.local(self.now).date()
    }
}

/// The first day of the period of `unit` that holds `day`.
fn start_of(day: NaiveDate, unit: &str) -> NaiveDate {
    match unit {
        "week" => day
            .checked_sub_signed(Duration::days(i64::from(
                day.weekday().num_days_from_monday(),
            )))
            .unwrap_or(day),
        "month" => day.with_day(1).unwrap_or(day),
        "quarter" => {
            NaiveDate::from_ymd_opt(day.year(), ((day.month() - 1) / 3) * 3 + 1, 1).unwrap_or(day)
        }
        "year" => NaiveDate::from_ymd_opt(day.year(), 1, 1).unwrap_or(day),
        _ => day,
    }
}

/// The day after the period of `unit` that starts on `day`; `None` past the dates chrono
/// holds.
fn next_period(day: NaiveDate, unit: &str) -> Option<NaiveDate> {
    match unit {
        "week" => day.checked_add_signed(Duration::days(7)),
        "month" => day.checked_add_months(chrono::Months::new(1)),
        "quarter" => day.checked_add_months(chrono::Months::new(3)),
        "year" => day.checked_add_months(chrono::Months::new(12)),
        _ => day.succ_opt(),
    }
}

/// The furthest an offset (`-30d`, `+1y`) reaches, in years either way. The reference
/// publishes no bound; lsf's keeps every date it computes within what a date holds, and an
/// offset past it is not a date (`Syntax Error: '-99999999999d' is not a date`).
pub const MAX_OFFSET_YEARS: i64 = 10_000;

/// `-30d`, `+2w`, `-3m`, `-1q`, `-1y`: the unit and the count, within [`MAX_OFFSET_YEARS`].
fn offset(text: &str) -> Option<(i64, &'static str)> {
    let (number, unit) = [
        ("min", "minute"),
        ("h", "hour"),
        ("d", "day"),
        ("w", "week"),
        ("m", "month"),
        ("q", "quarter"),
        ("y", "year"),
    ]
    .iter()
    .find_map(|(suffix, unit)| text.strip_suffix(suffix).map(|number| (number, *unit)))?;
    let n: i64 = number.parse().ok()?;
    let per_year: i64 = match unit {
        "minute" => 366 * 24 * 60,
        "hour" => 366 * 24,
        "day" => 366,
        "week" => 53,
        "month" => 12,
        "quarter" => 4,
        _ => 1,
    };
    (n.unsigned_abs() <= (MAX_OFFSET_YEARS * per_year).unsigned_abs()).then_some((n, unit))
}

/// A day moved by `n` units; `None` past the dates chrono holds.
fn shift(day: NaiveDate, n: i64, unit: &str) -> Option<NaiveDate> {
    let months = |months: i64| {
        let count = chrono::Months::new(u32::try_from(months.unsigned_abs()).ok()?);
        if months >= 0 {
            day.checked_add_months(count)
        } else {
            day.checked_sub_months(count)
        }
    };
    match unit {
        "week" => day.checked_add_signed(Duration::try_weeks(n)?),
        "month" => months(n),
        "quarter" => months(n.checked_mul(3)?),
        "year" => months(n.checked_mul(12)?),
        _ => day.checked_add_signed(Duration::try_days(n)?),
    }
}

/// A date bound: its first instant (`end`: its last).
fn bound(tokens: &[Tok], clock: &Clock, end: bool) -> Result<DateTime<Utc>, String> {
    let text: String = tokens.iter().map(Tok::written).collect::<Vec<_>>().join("");
    let invalid = || format!("Syntax Error: '{text}' is not a date");
    let day_bounds = |day: NaiveDate| -> Result<DateTime<Utc>, String> {
        let start = clock.utc(day.and_hms_opt(0, 0, 0).unwrap_or_default());
        Ok(if end {
            clock.utc(
                next_period(day, "day")
                    .ok_or_else(invalid)?
                    .and_hms_opt(0, 0, 0)
                    .unwrap_or_default(),
            ) - Duration::seconds(1)
        } else {
            start
        })
    };
    let lower = text.to_ascii_lowercase();
    if lower == "now" {
        return Ok(clock.now);
    }
    if lower == "today" {
        return if end {
            Ok(clock.now)
        } else {
            day_bounds(clock.today())
        };
    }
    if lower == "yesterday" {
        return day_bounds(clock.today().pred_opt().ok_or_else(invalid)?);
    }
    // `startOfMonth(-1m)`, `endOfDay(-1d)`.
    for (prefix, at_end) in [("startof", false), ("endof", true)] {
        if let Some(rest) = lower.strip_prefix(prefix) {
            let (unit, inner) = rest.split_once('(').ok_or_else(invalid)?;
            let inner = inner.strip_suffix(')').ok_or_else(invalid)?;
            let (n, _) = offset(inner).ok_or_else(invalid)?;
            let day = start_of(shift(clock.today(), n, unit).ok_or_else(invalid)?, unit);
            let start = clock.utc(day.and_hms_opt(0, 0, 0).unwrap_or_default());
            return Ok(if at_end {
                clock.utc(
                    next_period(day, unit)
                        .ok_or_else(invalid)?
                        .and_hms_opt(0, 0, 0)
                        .unwrap_or_default(),
                ) - Duration::seconds(1)
            } else {
                start
            });
        }
    }
    if let Some((n, unit)) = offset(&lower) {
        return match unit {
            "minute" => Duration::try_minutes(n)
                .and_then(|span| clock.now.checked_add_signed(span))
                .ok_or_else(invalid),
            "hour" => Duration::try_hours(n)
                .and_then(|span| clock.now.checked_add_signed(span))
                .ok_or_else(invalid),
            unit => day_bounds(shift(clock.today(), n, unit).ok_or_else(invalid)?),
        };
    }
    if let Ok(day) = NaiveDate::parse_from_str(&text, "%Y-%m-%d") {
        return day_bounds(day);
    }
    if let Ok(time) = NaiveDateTime::parse_from_str(&text, "%Y-%m-%dT%H:%M:%S") {
        return Ok(clock.utc(time));
    }
    Err(invalid())
}

/// The period a `DURING` names.
fn during(name: &str, clock: &Clock) -> Result<(DateTime<Utc>, DateTime<Utc>), String> {
    let today = clock.today();
    let invalid = || format!("Syntax Error: '{name}' is not a period");
    let span = |start: NaiveDate, end: Option<NaiveDate>| {
        let end = end.ok_or_else(invalid)?;
        Ok::<_, String>((
            clock.utc(start.and_hms_opt(0, 0, 0).unwrap_or_default()),
            clock.utc(end.and_hms_opt(0, 0, 0).unwrap_or_default()) - Duration::seconds(1),
        ))
    };
    let this = |unit: &str| {
        let start = start_of(today, unit);
        span(start, next_period(start, unit))
    };
    let last = |unit: &str| {
        let start = start_of(shift(today, -1, unit).ok_or_else(invalid)?, unit);
        span(start, next_period(start, unit))
    };
    match name {
        "today" => span(today, today.succ_opt()),
        "yesterday" => span(today.pred_opt().ok_or_else(invalid)?, Some(today)),
        "this_week" => this("week"),
        "this_month" => this("month"),
        "this_quarter" => this("quarter"),
        "this_year" => this("year"),
        "last_week" => last("week"),
        "last_month" => last("month"),
        "last_quarter" => last("quarter"),
        "last_year" => last("year"),
        name if name.starts_with("bfcm") => {
            // Black Friday (the day after the fourth Thursday of November) to Cyber Monday.
            let year: i32 = name[4..].parse().map_err(|_| invalid())?;
            let first = NaiveDate::from_ymd_opt(year, 11, 1).ok_or_else(invalid)?;
            let thursday = (3 - first.weekday().num_days_from_monday() as i64 + 7) % 7;
            let friday = first
                .checked_add_signed(Duration::days(thursday + 21 + 1))
                .ok_or_else(invalid)?;
            span(friday, friday.checked_add_signed(Duration::days(4)))
        }
        _ => Err(invalid()),
    }
}

// --- Sales ---

/// One row of the `sales` schema: a line sold, an order's shipping and taxes, or a return.
#[derive(Clone, Debug, Default)]
struct Sale {
    at: Option<DateTime<Utc>>,
    order: String,
    customer: Option<String>,
    first_order: bool,
    is_line: bool,
    dims: BTreeMap<&'static str, Value>,
    gross: f64,
    discounts: f64,
    returns: f64,
    shipping: f64,
    taxes: f64,
    quantity: f64,
    returned: f64,
}

fn money(value: Option<&Value>) -> f64 {
    cents_of(
        value
            .and_then(|set| set.get("shopMoney").or(Some(set)))
            .and_then(|money| money.get("amount")),
    )
    .map_or(0.0, |cents| cents as f64 / 100.0)
}

fn channel(source: Option<&str>) -> &'static str {
    match source {
        Some("web") | None => "Online Store",
        Some("pos") => "Point of Sale",
        Some("shopify_draft_order") => "Draft Orders",
        Some(_) => "Other",
    }
}

fn sales(state: &ShopState) -> Vec<Sale> {
    let mut first_orders: std::collections::HashMap<String, (DateTime<Utc>, String)> =
        std::collections::HashMap::new();
    for order in state.all("Order") {
        let (Some(customer), Some(at), Some(id)) = (
            order.get("customer").and_then(Value::as_str),
            value::date_time_of(order.get("processedAt")),
            order.get("id").and_then(Value::as_str),
        ) else {
            continue;
        };
        let entry = first_orders
            .entry(customer.to_string())
            .or_insert((at, id.to_string()));
        if at < entry.0 {
            *entry = (at, id.to_string());
        }
    }
    let mut rows = Vec::new();
    for order in state.all("Order") {
        let id = order
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let at = value::date_time_of(order.get("processedAt").or_else(|| order.get("createdAt")));
        let customer = order
            .get("customer")
            .and_then(Value::as_str)
            .map(str::to_string);
        let first = customer
            .as_ref()
            .and_then(|customer| first_orders.get(customer))
            .is_some_and(|(_, first)| *first == id);
        let customer_record = customer.as_deref().and_then(|customer| state.get(customer));
        let address = |field: &str, part: &str| {
            order
                .get(field)
                .and_then(|address| address.get(part))
                .cloned()
                .unwrap_or(Value::Null)
        };
        let mut dims: BTreeMap<&'static str, Value> = BTreeMap::new();
        dims.insert(
            "order_id",
            json!(crate::state::parse_gid(&id).map(|(_, n)| n)),
        );
        dims.insert(
            "order_name",
            order.get("name").cloned().unwrap_or(Value::Null),
        );
        dims.insert(
            "sales_channel",
            json!(channel(order.get("sourceName").and_then(Value::as_str))),
        );
        dims.insert(
            "currency",
            order.get("currencyCode").cloned().unwrap_or(Value::Null),
        );
        dims.insert(
            "customer_id",
            json!(
                customer
                    .as_deref()
                    .and_then(crate::state::parse_gid)
                    .map(|(_, n)| n)
            ),
        );
        dims.insert(
            "customer_name",
            customer_record
                .and_then(|record| record.get("displayName"))
                .cloned()
                .unwrap_or(Value::Null),
        );
        dims.insert(
            "customer_email",
            customer_record
                .and_then(|record| record.get("email"))
                .cloned()
                .unwrap_or(Value::Null),
        );
        dims.insert(
            "new_or_returning_customer",
            match &customer {
                Some(_) if first => json!("New"),
                Some(_) => json!("Returning"),
                None => Value::Null,
            },
        );
        dims.insert("billing_country", address("billingAddress", "country"));
        dims.insert("billing_city", address("billingAddress", "city"));
        dims.insert("shipping_country", address("shippingAddress", "country"));
        dims.insert("shipping_city", address("shippingAddress", "city"));
        dims.insert(
            "discount_code",
            order.get("discountCode").cloned().unwrap_or(Value::Null),
        );
        let base = Sale {
            at,
            order: id.clone(),
            customer: customer.clone(),
            first_order: first,
            dims: dims.clone(),
            ..Sale::default()
        };
        let mut line_discounts = 0.0;
        for line in order
            .get("lineItems")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .filter_map(|line| state.get(line))
        {
            let gross = money(line.get("originalTotalSet"));
            let discounted = line
                .get("discountedTotalSet")
                .map_or(gross, |set| money(Some(set)));
            let discount = (gross - discounted).max(0.0);
            line_discounts += discount;
            let product = line
                .get("product")
                .and_then(Value::as_str)
                .and_then(|p| state.get(p));
            let variant = line
                .get("variant")
                .and_then(Value::as_str)
                .and_then(|v| state.get(v));
            let mut row = base.clone();
            row.is_line = true;
            row.gross = gross;
            row.discounts = -discount;
            row.quantity = line.get("quantity").and_then(Value::as_f64).unwrap_or(0.0);
            let id_of = |record: Option<&Record>| {
                json!(
                    record
                        .and_then(|record| record.get("id"))
                        .and_then(Value::as_str)
                        .and_then(crate::state::parse_gid)
                        .map(|(_, n)| n)
                )
            };
            let field = |record: Option<&Record>, name: &str| {
                record
                    .and_then(|record| record.get(name))
                    .cloned()
                    .unwrap_or(Value::Null)
            };
            row.dims.insert("product_id", id_of(product));
            row.dims.insert(
                "product_title",
                product.map_or_else(|| field(Some(line), "title"), |p| field(Some(p), "title")),
            );
            row.dims
                .insert("product_type", field(product, "productType"));
            row.dims.insert("product_vendor", field(product, "vendor"));
            row.dims.insert("product_variant_id", id_of(variant));
            row.dims
                .insert("product_variant_title", field(variant, "title"));
            row.dims
                .insert("product_variant_sku", field(variant, "sku"));
            rows.push(row);
        }
        let mut row = base.clone();
        row.shipping = money(order.get("totalShippingPriceSet"));
        row.taxes = money(order.get("totalTaxSet"));
        let total_discounts = money(order.get("totalDiscountsSet"));
        row.discounts = -(total_discounts - line_discounts).max(0.0);
        rows.push(row);
    }
    for refund in state.all("Refund") {
        let order_id = refund
            .get("order")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let Some(order) = state.get(order_id) else {
            continue;
        };
        let mut row = Sale {
            at: value::date_time_of(refund.get("createdAt")),
            order: order_id.to_string(),
            customer: order
                .get("customer")
                .and_then(Value::as_str)
                .map(str::to_string),
            ..Sale::default()
        };
        let lines: Vec<&Value> = refund
            .get("refundLineItems")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .collect();
        let subtotal: f64 = lines
            .iter()
            .map(|line| money(line.get("subtotalSet")))
            .sum();
        let quantity: f64 = lines
            .iter()
            .filter_map(|line| line.get("quantity").and_then(Value::as_f64))
            .sum();
        row.returns = -subtotal;
        row.returned = quantity;
        row.dims.insert(
            "order_id",
            json!(crate::state::parse_gid(order_id).map(|(_, n)| n)),
        );
        row.dims.insert(
            "order_name",
            order.get("name").cloned().unwrap_or(Value::Null),
        );
        rows.push(row);
    }
    rows
}

/// The value of a time dimension of an instant, as Shopify writes it (`2026-01-01`).
fn time_value(at: DateTime<Utc>, unit: &str, clock: &Clock) -> Value {
    let local = clock.local(at);
    let day = local.date();
    match unit {
        "hour" => json!(format!("{}T{:02}:00:00", day, local.time().format("%H"))),
        "day_of_week" => json!(day.weekday().num_days_from_monday()),
        "month_of_year" => json!(day.month()),
        unit => json!(start_of(day, unit).to_string()),
    }
}

/// The value of a row's dimension.
fn dim_value(sale: &Sale, name: &str, clock: &Clock) -> Value {
    if TIME.contains(&name) || name == "day_of_week" || name == "month_of_year" {
        return sale
            .at
            .map_or(Value::Null, |at| time_value(at, name, clock));
    }
    sale.dims.get(name).cloned().unwrap_or(Value::Null)
}

/// Whether a row passes a condition.
fn passes(condition: &Cond, sale: &Sale, clock: &Clock) -> bool {
    match condition {
        Cond::And(parts) => parts.iter().all(|part| passes(part, sale, clock)),
        Cond::Or(parts) => parts.iter().any(|part| passes(part, sale, clock)),
        Cond::Not(inner) => !passes(inner, sale, clock),
        Cond::Compare(field, operator, values) => {
            let value = dim_value(sale, field, clock);
            let text = |value: &Value| match value {
                Value::String(text) => Some(text.clone()),
                Value::Number(number) => Some(number.to_string()),
                _ => None,
            };
            let Some(have) = text(&value) else {
                return operator == "!=" || operator == "NOT IN";
            };
            let wanted: Vec<String> = values
                .iter()
                .map(|token| match token {
                    Tok::Text(text) | Tok::Number(text) | Tok::Word(text) => text.clone(),
                    Tok::Symbol(text) => text.clone(),
                })
                .collect();
            let compare = |want: &str| match (have.parse::<f64>(), want.parse::<f64>()) {
                (Ok(a), Ok(b)) => a.partial_cmp(&b),
                _ => Some(have.to_lowercase().cmp(&want.to_lowercase())),
            };
            use std::cmp::Ordering::*;
            match operator.as_str() {
                "IN" => wanted.iter().any(|want| compare(want) == Some(Equal)),
                "NOT IN" => !wanted.iter().any(|want| compare(want) == Some(Equal)),
                op => wanted.first().is_some_and(|want| {
                    let ordering = compare(want);
                    match op {
                        "=" => ordering == Some(Equal),
                        "!=" => ordering != Some(Equal),
                        "<" => ordering == Some(Less),
                        "<=" => matches!(ordering, Some(Less | Equal)),
                        ">" => ordering == Some(Greater),
                        ">=" => matches!(ordering, Some(Greater | Equal)),
                        _ => false,
                    }
                }),
            }
        }
    }
}

/// The metrics of a group of rows.
fn metrics(rows: &[&Sale]) -> BTreeMap<&'static str, f64> {
    let sum = |read: fn(&Sale) -> f64| rows.iter().map(|row| read(row)).sum::<f64>();
    let gross = sum(|row| row.gross);
    let discounts = sum(|row| row.discounts);
    let returns = sum(|row| row.returns);
    let shipping = sum(|row| row.shipping);
    let taxes = sum(|row| row.taxes);
    let net = gross + discounts + returns;
    let placed: Vec<&&Sale> = rows
        .iter()
        .filter(|row| row.returns == 0.0 && row.returned == 0.0)
        .collect();
    let orders: HashSet<&str> = placed.iter().map(|row| row.order.as_str()).collect();
    let customers: HashSet<&str> = placed
        .iter()
        .filter_map(|row| row.customer.as_deref())
        .collect();
    let new_customers: HashSet<&str> = placed
        .iter()
        .filter(|row| row.first_order)
        .filter_map(|row| row.customer.as_deref())
        .collect();
    let first_orders: HashSet<&str> = placed
        .iter()
        .filter(|row| row.first_order)
        .map(|row| row.order.as_str())
        .collect();
    let quantity = sum(|row| row.quantity);
    let returned = sum(|row| row.returned);
    let mut values = BTreeMap::new();
    values.insert("gross_sales", gross);
    values.insert("discounts", discounts);
    values.insert("returns", returns);
    values.insert("net_sales", net);
    values.insert("shipping_charges", shipping);
    values.insert("taxes", taxes);
    values.insert("total_sales", net + shipping + taxes);
    values.insert("orders", orders.len() as f64);
    values.insert(
        "average_order_value",
        if orders.is_empty() {
            0.0
        } else {
            (gross + discounts) / orders.len() as f64
        },
    );
    values.insert("quantity_ordered", quantity);
    values.insert("quantity_returned", returned);
    values.insert("net_items_sold", quantity - returned);
    values.insert("customers", customers.len() as f64);
    values.insert("new_customers", new_customers.len() as f64);
    values.insert(
        "returning_customers",
        (customers.len() - new_customers.len().min(customers.len())) as f64,
    );
    values.insert("orders_first_time", first_orders.len() as f64);
    values.insert(
        "orders_returning",
        (orders.len() - first_orders.len().min(orders.len())) as f64,
    );
    values
}

/// A metric's value as the table writes it: money as a string of two decimals, an integer as
/// a number.
fn cell(kind: &str, value: f64) -> Value {
    match kind {
        "MONEY" => json!(format!("{value:.2}")),
        "INTEGER" => json!(value.round() as i64),
        _ => json!(value),
    }
}

fn column(name: &str, kind: &str, display: &str, origin: &str, totals_of: Option<&str>) -> Value {
    json!({
        "name": name,
        "dataType": kind,
        "displayName": display,
        "shortDisplayName": null,
        "subType": null,
        "columnOrigin": origin,
        "dynamicColumnMetadata": totals_of.map(|original| json!({
            "type": "TOTALS",
            "aggregatedBy": [],
            "comparisonReference": null,
            "originalColumnName": original,
        })),
    })
}

/// Runs a query: the table, or the parse errors.
fn run(state: &ShopState, text: &str) -> Result<(Value, Vec<String>), Vec<String>> {
    let query = parse(text).map_err(|error| vec![error])?;
    if query.from != "sales" {
        return Err(vec![format!(
            "Table Not Found: Table '{}' not found (lsf queries the sales schema)",
            query.from
        )]);
    }
    let zone = state
        .shop()
        .and_then(|shop| shop.get("ianaTimezone"))
        .and_then(Value::as_str)
        .and_then(|zone| zone.parse::<Tz>().ok())
        .unwrap_or(Tz::UTC);
    let clock = Clock {
        now: state.now(),
        zone,
    };
    let mut dims: Vec<String> = Vec::new();
    if let Some(unit) = &query.timeseries {
        if !TIME.contains(&unit.as_str()) && unit != "day_of_week" && unit != "month_of_year" {
            return Err(vec![not_found(unit)]);
        }
        dims.push(unit.clone());
    }
    for name in &query.group_by {
        if dimension(name).is_none() {
            return Err(vec![not_found(name)]);
        }
        if !dims.contains(name) {
            dims.push(name.clone());
        }
    }
    let mut shown: Vec<&'static (&'static str, &'static str, &'static str)> = Vec::new();
    for name in &query.show {
        if dimension(name).is_some() {
            if !dims.contains(name) {
                dims.push(name.clone());
            }
            continue;
        }
        let Some(found) = metric(name) else {
            return Err(vec![not_found(name)]);
        };
        if !COMPUTED.contains(&found.0) {
            return Err(vec![format!(
                "Column Not Found: Column '{name}' is a metric lsf does not compute"
            )]);
        }
        shown.push(found);
    }
    if let Some(condition) = &query.condition {
        check_fields(condition).map_err(|error| vec![error])?;
    }
    // The period.
    let (since, until) = match (&query.during, &query.since, &query.until) {
        (Some(name), _, _) => {
            let (start, end) = during(name, &clock).map_err(|error| vec![error])?;
            (Some(start), Some(end))
        }
        (None, since, until) => {
            let since = since
                .as_ref()
                .map(|tokens| bound(tokens, &clock, false))
                .transpose()
                .map_err(|error| vec![error])?;
            let until = until
                .as_ref()
                .map(|tokens| bound(tokens, &clock, true))
                .transpose()
                .map_err(|error| vec![error])?;
            let default_since = || {
                let unit = query.timeseries.as_deref()?;
                let (n, unit) = match unit {
                    "hour" => (0, "day"),
                    "day" => (-30, "day"),
                    "week" | "day_of_week" => (-4, "week"),
                    "month" => (-3, "month"),
                    "quarter" => (-4, "quarter"),
                    _ => (if unit == "year" { -3 } else { -1 }, "year"),
                };
                let day = start_of(shift(clock.today(), n, unit)?, unit);
                Some(clock.utc(day.and_hms_opt(0, 0, 0).unwrap_or_default()))
            };
            let since = since.or_else(default_since);
            let until = until.or(since.map(|_| clock.now));
            (since, until)
        }
    };
    if let (Some(since), Some(until)) = (since, until)
        && until < since
    {
        return Err(vec!["Syntax Error: UNTIL can't be before SINCE".into()]);
    }
    let product_grouped = dims
        .iter()
        .any(|dim| PRODUCT_DIMENSIONS.contains(&dim.as_str()));
    let all = sales(state);
    let kept: Vec<&Sale> = all
        .iter()
        .filter(|sale| {
            let in_period = sale.at.is_some_and(|at| {
                since.is_none_or(|since| at >= since) && until.is_none_or(|until| at <= until)
            });
            in_period
                && (!product_grouped || sale.is_line || sale.returned > 0.0)
                && query
                    .condition
                    .as_ref()
                    .is_none_or(|condition| passes(condition, sale, &clock))
        })
        .collect();
    // Groups, in the order the dimensions sort.
    let mut groups: BTreeMap<String, (Vec<Value>, Vec<&Sale>)> = BTreeMap::new();
    for sale in &kept {
        let key_values: Vec<Value> = dims
            .iter()
            .map(|dim| dim_value(sale, dim, &clock))
            .collect();
        let key = serde_json::to_string(&key_values).unwrap_or_default();
        groups
            .entry(key)
            .or_insert_with(|| (key_values, Vec::new()))
            .1
            .push(sale);
    }
    // TIMESERIES fills the periods without sales, when it is the only dimension.
    if let (Some(unit), Some(since), Some(until)) = (&query.timeseries, since, until)
        && dims.len() == 1
        && TIME.contains(&unit.as_str())
        && unit != "hour"
    {
        let mut day = start_of(clock.local(since).date(), unit);
        let last = clock.local(until).date();
        while day <= last {
            let key_values = vec![json!(day.to_string())];
            let key = serde_json::to_string(&key_values).unwrap_or_default();
            groups
                .entry(key)
                .or_insert_with(|| (key_values, Vec::new()));
            let Some(next) = next_period(day, unit) else {
                break;
            };
            day = next;
        }
    }
    let totals = metrics(&kept);
    let mut columns: Vec<Value> = dims
        .iter()
        .map(|dim| {
            let (name, kind, display) =
                dimension(dim)
                    .copied()
                    .unwrap_or((dim.as_str(), "STRING", dim.as_str()));
            let _ = name;
            column(dim, kind, display, "REQUESTED", None)
        })
        .collect();
    for (name, kind, display) in &shown {
        columns.push(column(name, kind, display, "REQUESTED", None));
    }
    if query.totals {
        for (name, kind, display) in &shown {
            columns.push(column(
                &format!("{name}__totals"),
                kind,
                &format!("{display} (totals)"),
                "GENERATED",
                Some(name),
            ));
        }
    }
    let mut rows: Vec<Map<String, Value>> = groups
        .into_values()
        .map(|(key_values, sales)| {
            let values = metrics(&sales);
            let mut row = Map::new();
            for (dim, value) in dims.iter().zip(key_values) {
                row.insert(dim.clone(), value);
            }
            for (name, kind, _) in &shown {
                row.insert(
                    name.to_string(),
                    cell(kind, values.get(name).copied().unwrap_or(0.0)),
                );
            }
            if query.totals {
                for (name, kind, _) in &shown {
                    row.insert(
                        format!("{name}__totals"),
                        cell(kind, totals.get(name).copied().unwrap_or(0.0)),
                    );
                }
            }
            row
        })
        .collect();
    if rows.is_empty() && dims.is_empty() {
        let mut row = Map::new();
        for (name, kind, _) in &shown {
            row.insert(name.to_string(), cell(kind, 0.0));
        }
        rows.push(row);
    }
    // ORDER BY, in its order; the groups sort by their dimensions otherwise.
    for (name, _) in &query.order_by {
        let known = dims.contains(name)
            || shown.iter().any(|(metric, _, _)| metric == name)
            || (query.totals
                && shown
                    .iter()
                    .any(|(metric, _, _)| format!("{metric}__totals") == *name));
        if !known {
            return Err(vec![not_found(name)]);
        }
    }
    if !query.order_by.is_empty() {
        let order = query.order_by.clone();
        rows.sort_by(|a, b| {
            for (name, descending) in &order {
                let number = |row: &Map<String, Value>| match row.get(name) {
                    Some(Value::Number(n)) => n.as_f64(),
                    Some(Value::String(text)) => text.parse::<f64>().ok(),
                    _ => None,
                };
                let ordering = match (number(a), number(b)) {
                    (Some(x), Some(y)) => x.partial_cmp(&y).unwrap_or(std::cmp::Ordering::Equal),
                    _ => {
                        let text = |row: &Map<String, Value>| {
                            row.get(name).map(|v| v.to_string()).unwrap_or_default()
                        };
                        text(a).cmp(&text(b))
                    }
                };
                let ordering = if *descending {
                    ordering.reverse()
                } else {
                    ordering
                };
                if ordering != std::cmp::Ordering::Equal {
                    return ordering;
                }
            }
            std::cmp::Ordering::Equal
        });
    }
    if let Some(limit) = query.limit {
        rows.truncate(limit);
    }
    let row_metadata: Vec<Value> = rows
        .iter()
        .map(|_| json!({"nullCellTranslations": [], "rawResourceIds": [], "topNRemainderColumnNames": []}))
        .collect();
    let table = json!({
        "columns": columns,
        "rows": rows,
        "rowMetadata": row_metadata,
    });
    Ok((table, query.ignored))
}

/// `shopifyqlQuery(query:)`.
pub fn resolve(
    r: &ShopResolver<'_>,
    ctx: &FieldContext<'_>,
    _object: &Obj,
) -> Result<Output<Obj>, FieldError> {
    let text = ctx.arg_str("query").unwrap_or_default();
    let state = r.state();
    let normalized = |text: &str| text.split_whitespace().collect::<Vec<_>>().join(" ");
    let targets: Vec<Value> = state
        .all("AnalyticsTarget")
        .filter(|target| {
            target
                .get("shopifyqlQuery")
                .and_then(Value::as_str)
                .is_some_and(|query| normalized(query) == normalized(text))
        })
        .filter_map(|target| target.get("id").cloned())
        .collect();
    let mut value = Record::new();
    match run(state, text) {
        Ok((table, ignored)) => {
            for clause in ignored {
                r.note(format!(
                    "shopifyqlQuery: lsf answers the table without {clause}"
                ));
            }
            value.insert("tableData".into(), table);
            value.insert("parseErrors".into(), json!([]));
        }
        Err(errors) => {
            value.insert("tableData".into(), Value::Null);
            value.insert("parseErrors".into(), json!(errors));
        }
    }
    value.insert("parseWarnings".into(), json!([]));
    value.insert("analyticsTargets".into(), json!(targets));
    let object = Obj::built("ShopifyqlQueryResponse", value);
    Ok(Output::Object(object))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_queries_of_the_reference() {
        for query in [
            "FROM sales SHOW total_sales GROUP BY month SINCE -3m ORDER BY month",
            "FROM sales SHOW total_sales GROUP BY month SINCE -3m ORDER BY month WITH TOTALS",
            "FROM sales SHOW total_sales, orders TIMESERIES month SINCE -12m UNTIL today",
            "FROM sales SHOW total_sales DURING last_month",
            "FROM sales SHOW total_sales TIMESERIES day SINCE -30d UNTIL today",
            "FROM sales SHOW total_sales GROUP BY sales_channel DURING last_month ORDER BY total_sales DESC",
            "FROM sales SHOW net_sales, orders TIMESERIES day SINCE startOfDay(-30d) UNTIL endOfDay(-1d)",
            "FROM sales SHOW total_sales WHERE sales_channel = 'Online Store' GROUP BY product_title DURING last_month ORDER BY total_sales DESC LIMIT 10 VISUALIZE total_sales TYPE horizontal_bar",
        ] {
            parse(query).unwrap_or_else(|error| panic!("{query}: {error}"));
        }
        assert!(parse("SHOW total_sales").is_err());
        assert!(check_condition("sales_channel = 'Online Store'").is_ok());
        assert_eq!(
            check_condition("channel = 'x'").unwrap_err(),
            "Column Not Found: Column 'channel' not found"
        );
    }

    #[test]
    fn a_long_or_deep_condition_is_read_or_refused_without_overflowing() {
        // Wide: a long chain of `AND` and `OR`, and of `NOT` (counted: an even number is none).
        let wide = vec!["sales_channel = 'Online Store'"; 100_000].join(" AND ");
        assert!(check_condition(&wide).is_ok());
        let alternatives = vec!["currency = 'EUR'"; 100_000].join(" OR ");
        assert!(check_condition(&alternatives).is_ok());
        let negated = format!("{}currency = 'EUR'", "NOT ".repeat(400_000));
        assert!(check_condition(&negated).is_ok());
        let mut parser = Parser {
            tokens: lex(&format!("{}currency = 'EUR'", "NOT ".repeat(3))).unwrap(),
            at: 0,
            depth: 0,
        };
        assert!(
            matches!(parser.condition(), Ok(Cond::Not(inner)) if matches!(*inner, Cond::Compare(..)))
        );
        // Deep: parentheses, a hundred at most.
        let nested =
            |depth: usize| format!("{}currency = 'EUR'{}", "(".repeat(depth), ")".repeat(depth));
        assert!(check_condition(&nested(MAX_DEPTH)).is_ok());
        let refused = "Syntax Error: parentheses nest more than 100 deep";
        assert_eq!(
            check_condition(&nested(MAX_DEPTH + 1)).unwrap_err(),
            refused
        );
        assert_eq!(check_condition(&"(".repeat(400_000)).unwrap_err(), refused);
        let query = format!("FROM sales SHOW orders WHERE {}", "(".repeat(400_000));
        assert_eq!(parse(&query).unwrap_err(), refused);
    }

    #[test]
    fn periods_are_the_shop_calendar() {
        let clock = Clock {
            now: Utc.with_ymd_and_hms(2026, 10, 31, 12, 0, 0).unwrap(),
            zone: Tz::UTC,
        };
        let (start, end) = during("last_month", &clock).unwrap();
        assert_eq!(start.to_rfc3339(), "2026-09-01T00:00:00+00:00");
        assert_eq!(end.to_rfc3339(), "2026-09-30T23:59:59+00:00");
        let (friday, _) = during("bfcm2025", &clock).unwrap();
        assert_eq!(friday.date_naive().to_string(), "2025-11-28");
        let since = bound(&lex("-3m").unwrap(), &clock, false).unwrap();
        assert_eq!(since.to_rfc3339(), "2026-07-31T00:00:00+00:00");
        assert_eq!(
            start_of(NaiveDate::from_ymd_opt(2026, 10, 31).unwrap(), "week").to_string(),
            "2026-10-26"
        );
    }
}
