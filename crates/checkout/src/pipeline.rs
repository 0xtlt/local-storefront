//! Computes the checkout: runs the Functions of the app, in the order Shopify runs them, and
//! applies what they return.
//!
//! Cart transform → product and order discounts → delivery groups → delivery customization →
//! shipping discounts → payment customization → validation → cost.
//!
//! What is not supported: fulfillment constraints, order routing and the pickup generators
//! (listed by [`crate::app`], not run), the `.fetch` targets of discounts, discount
//! `conditions` on candidates (every candidate applies), combinations between discounts (they
//! all stack), tax lines (one rate on the discounted subtotal) and prices by market.
//!
//! Each Function runs at the API version it declares: its input query is checked against the
//! schema of that version, and its output has to fit the result type of its target in that
//! schema (operations of the current targets, `FunctionRunResult` of the legacy `purchase.*`
//! ones). A Function whose version is not supported is not run.

use std::collections::BTreeSet;

use chrono::{DateTime, FixedOffset, Utc};
use lsf_core::drops::cart::ResolvedLine;
use lsf_core::store::CartLine;
use serde_json::Value;

use crate::app::{App, FunctionExtension, FunctionTarget, Selection, TargetKind};
use crate::input::{InputContext, Owner, OwnerKind};
use crate::runner;
use crate::version::ApiVersion;
use crate::{
    AppliedDiscount, Cents, CheckoutConfig, CheckoutError, CheckoutResult, CheckoutState, Cost,
    DeliveryGroup, DeliveryOption, DiscountAllocation, DiscountClass, DiscountCodeStatus,
    DiscountConfig, FunctionRun, KeyValue, Metafield, PaymentMethod, ResultLine, SourceLine,
};

/// The handle of the delivery group: one group holds every line that needs shipping.
pub const DELIVERY_GROUP_HANDLE: &str = "group-1";

/// Builds the input of a Function: [`crate::input::build`], or another builder for tests.
pub type InputBuilder<'a> =
    dyn Fn(&str, Option<&Value>, &InputContext) -> Result<Value, String> + 'a;

/// Runs every enabled Function against the cart of `session` and describes the checkout.
///
/// Functions that fail (no module, a trap, an output that does not parse, more than 11 million
/// instructions) do not stop the checkout: the failure is in the [`FunctionRun`] and in
/// [`CheckoutResult::errors`], which block completing the order.
///
/// What the input builder can rely on: the delivery group is [`DELIVERY_GROUP_HANDLE`], and
/// outputs name a delivery group or a payment method by its handle or by a gid that ends with
/// it (`gid://shopify/CartDeliveryGroup/group-1`), so the `id` the builder gives them must do
/// the same. Cart lines are named by the `id` of [`ResultLine`], product variants by their gid.
pub fn compute(
    app: &App,
    enabled: &Selection,
    store: &lsf_core::Store,
    session: &lsf_core::Session,
    state: &CheckoutState,
    config: &CheckoutConfig,
) -> CheckoutResult {
    compute_with(
        app,
        enabled,
        store,
        session,
        state,
        config,
        &crate::input::build,
    )
}

/// [`compute`] with another way to build the input of the Functions.
pub fn compute_with(
    app: &App,
    enabled: &Selection,
    store: &lsf_core::Store,
    session: &lsf_core::Session,
    state: &CheckoutState,
    config: &CheckoutConfig,
    build: &InputBuilder,
) -> CheckoutResult {
    let now = store
        .now
        .unwrap_or_else(Utc::now)
        .with_timezone(&store.shop.timezone)
        .fixed_offset();
    let pipeline = Pipeline {
        app,
        enabled,
        store,
        session,
        state,
        config,
        build,
        currency: store.shop.currency.clone(),
        now,
        lines: initial_lines(store, session),
        next_line_id: session.cart_lines.len() as u64 + 1,
        discounts: Vec::new(),
        accepted_codes: Vec::new(),
        rejected_codes: Vec::new(),
        groups: Vec::new(),
        payment_methods: config
            .payment_methods
            .iter()
            .map(|method| PaymentMethod {
                handle: method.handle.clone(),
                name: method.name.clone(),
            })
            .collect(),
        errors: Vec::new(),
        runs: Vec::new(),
    };
    pipeline.run()
}

struct Pipeline<'a> {
    app: &'a App,
    enabled: &'a Selection,
    store: &'a lsf_core::Store,
    session: &'a lsf_core::Session,
    state: &'a CheckoutState,
    config: &'a CheckoutConfig,
    build: &'a InputBuilder<'a>,
    currency: String,
    now: DateTime<FixedOffset>,
    lines: Vec<ResultLine>,
    next_line_id: u64,
    discounts: Vec<AppliedDiscount>,
    /// Entered codes a Function took (lowercase).
    accepted_codes: Vec<String>,
    /// Entered codes a Function turned down (lowercase).
    rejected_codes: Vec<String>,
    groups: Vec<DeliveryGroup>,
    payment_methods: Vec<PaymentMethod>,
    errors: Vec<CheckoutError>,
    runs: Vec<FunctionRun>,
}

/// What owns a Function run: the object whose metafields configure it.
struct OwnerSpec<'a> {
    kind: OwnerKind,
    id: String,
    metafields: &'a [Metafield],
    classes: &'a [DiscountClass],
}

impl<'a> Pipeline<'a> {
    fn run(mut self) -> CheckoutResult {
        self.cart_transforms();
        self.line_discounts();
        self.build_delivery_groups();
        self.delivery_customizations();
        self.shipping_discounts();
        self.payment_customizations();
        self.validations();
        self.finish()
    }

    /// The enabled Functions with a target of one of `kinds`, with those targets.
    fn targets_of(
        &self,
        function: &'a FunctionExtension,
        kinds: &[TargetKind],
    ) -> Vec<&'a FunctionTarget> {
        function
            .targets
            .iter()
            .filter(|target| kinds.contains(&target.kind))
            .collect()
    }

    fn functions_with(&self, kinds: &[TargetKind]) -> Vec<(usize, &'a FunctionExtension)> {
        let app = self.app;
        let enabled = self.enabled;
        app.functions
            .iter()
            .enumerate()
            .filter(|(_, function)| enabled.contains(&function.handle))
            .filter(|(_, function)| function.version.is_some())
            .filter(|(_, function)| {
                function
                    .targets
                    .iter()
                    .any(|target| kinds.contains(&target.kind))
            })
            .collect()
    }

    fn function_metafields(&self, handle: &str) -> &'a [Metafield] {
        let config = self.config;
        config
            .function_metafields
            .get(handle)
            .map_or(&[], Vec::as_slice)
    }

    /// Builds the input of one target, runs it and records the run. `None` when the run
    /// failed (the failure is in the checkout errors).
    fn execute(
        &mut self,
        function: &FunctionExtension,
        target: &FunctionTarget,
        owner: OwnerSpec,
        triggering_code: Option<&str>,
    ) -> Option<Value> {
        let (Some(version), Some(spec)) = (function.version, target.spec()) else {
            self.record_failure(
                function,
                target,
                Value::Null,
                format!("`{}` is not run by the local checkout", target.target),
            );
            return None;
        };
        let variables = match owner_variables(function, owner.metafields) {
            Ok(variables) => variables,
            Err(error) => {
                self.record_failure(function, target, Value::Null, error);
                return None;
            }
        };
        let entered_codes: Vec<String> = self.state.discount_codes.clone();
        let input = {
            let context = InputContext {
                version,
                api: spec.api,
                store: self.store,
                session: self.session,
                state: self.state,
                lines: &self.lines,
                delivery_groups: &self.groups,
                payment_methods: &self.payment_methods,
                owner: Owner {
                    kind: owner.kind,
                    id: owner.id,
                    metafields: owner.metafields,
                    discount_classes: owner.classes,
                },
                entered_codes: &entered_codes,
                triggering_code,
                currency: &self.currency,
                now: self.now,
            };
            (self.build)(&target.input_query, variables.as_ref(), &context)
        };
        let input = match input {
            Ok(input) => input,
            Err(error) => {
                self.record_failure(
                    function,
                    target,
                    Value::Null,
                    format!("cannot build the input: {error}"),
                );
                return None;
            }
        };

        let mut execution = runner::run(&function.wasm_path, &target.export, &input);
        if execution.error.is_none()
            && let Some(output) = &execution.output
            && let Err(problems) = version
                .schema(spec.api)
                .validate_output(spec.result, output)
        {
            execution.error = Some(format!(
                "the output does not fit `{}` in API version {version}: {problems}",
                spec.result
            ));
        }
        // Shopify stops a Function that runs past the limit: its output does not count.
        if execution.instructions > runner::INSTRUCTIONS_LIMIT {
            let message = format!(
                "used {} instructions: Shopify stops a Function past {}",
                execution.instructions,
                runner::INSTRUCTIONS_LIMIT
            );
            execution.error = Some(match execution.error.take() {
                Some(previous) => format!("{previous}; {message}"),
                None => message,
            });
        }
        let failed = execution.error.is_some() || execution.output.is_none();
        // What is applied has the lists the schema asks for; the run keeps what the Function
        // answered.
        let mut applied = execution.output.clone();
        if !failed && let Some(output) = &mut applied {
            version
                .schema(spec.api)
                .normalize_output(spec.result, output);
        }
        let run = FunctionRun {
            handle: function.handle.clone(),
            target: target.target.clone(),
            export: target.export.clone(),
            input,
            output: execution.output.clone(),
            logs: execution.logs,
            error: execution.error.clone(),
            instructions: execution.instructions,
        };
        if failed {
            self.errors.push(CheckoutError {
                message: format!(
                    "Function `{}` failed: {}",
                    function.handle,
                    execution.error.as_deref().unwrap_or("no output")
                ),
                target: None,
                function: Some(function.handle.clone()),
            });
        }
        self.runs.push(run);
        if failed { None } else { applied }
    }

    fn record_failure(
        &mut self,
        function: &FunctionExtension,
        target: &FunctionTarget,
        input: Value,
        error: String,
    ) {
        self.errors.push(CheckoutError {
            message: format!("Function `{}` failed: {error}", function.handle),
            target: None,
            function: Some(function.handle.clone()),
        });
        self.runs.push(FunctionRun {
            handle: function.handle.clone(),
            target: target.target.clone(),
            export: target.export.clone(),
            input,
            output: None,
            logs: String::new(),
            error: Some(error),
            instructions: 0,
        });
    }

    /// The output of the run that just ran could not be applied (rejected money, an operation
    /// that does not exist): the run fails, like a Function Shopify would reject the result of.
    /// Errors a Function reports on purpose (validations) do not come here.
    fn function_error(&mut self, function: &FunctionExtension, message: String) {
        if let Some(run) = self
            .runs
            .iter_mut()
            .rev()
            .find(|run| run.handle == function.handle)
        {
            run.error = Some(match run.error.take() {
                Some(previous) => format!("{previous}; {message}"),
                None => message.clone(),
            });
        }
        self.errors.push(CheckoutError {
            message: format!("Function `{}`: {message}", function.handle),
            target: None,
            function: Some(function.handle.clone()),
        });
    }

    // ---- cart transform ----

    fn cart_transforms(&mut self) {
        for (index, function) in self.functions_with(&[TargetKind::CartTransform]) {
            for target in self.targets_of(function, &[TargetKind::CartTransform]) {
                let owner = OwnerSpec {
                    kind: OwnerKind::CartTransform,
                    id: format!("gid://shopify/CartTransform/{}", index + 1),
                    metafields: self.function_metafields(&function.handle),
                    classes: &[],
                };
                if let Some(output) = self.execute(function, target, owner, None) {
                    self.apply_cart_transform(function, &output);
                }
            }
        }
        for line in &mut self.lines {
            refresh(line);
        }
    }

    fn apply_cart_transform(&mut self, function: &FunctionExtension, output: &Value) {
        for operation in array(output, "operations") {
            // A refused operation leaves nothing behind, not even the ids it took for new lines.
            let next_line_id = self.next_line_id;
            let result = if let Some(op) = operation.get("expand").or(operation.get("lineExpand")) {
                self.expand(op)
            } else if let Some(op) = operation.get("merge").or(operation.get("linesMerge")) {
                self.merge(op)
            } else if let Some(op) = operation.get("update").or(operation.get("lineUpdate")) {
                self.update(op)
            } else {
                Err("unknown operation".to_string())
            };
            if let Err(message) = result {
                self.next_line_id = next_line_id;
                self.function_error(function, format!("cart transform: {message}"));
            }
        }
    }

    fn line_index(&self, id: &str) -> Result<usize, String> {
        self.lines
            .iter()
            .position(|line| line.id == id)
            .ok_or_else(|| format!("no cart line `{id}`"))
    }

    /// The index of a line a cart transform operation changes. Shopify rejects expand, merge
    /// and update on a line with a selling plan.
    fn transformable_line(&self, id: &str, operation: &str) -> Result<usize, String> {
        let index = self.line_index(id)?;
        if self.has_selling_plan(&self.lines[index]) {
            return Err(format!(
                "cannot {operation} the cart line `{id}`: it has a selling plan"
            ));
        }
        Ok(index)
    }

    /// Whether a cart line the line is made of is sold with a selling plan the store knows (the
    /// plan that priced it).
    fn has_selling_plan(&self, line: &ResultLine) -> bool {
        line.sources.iter().any(|source| {
            let Some(cart_line) = self.session.cart_lines.get(source.index) else {
                return false;
            };
            let Some(plan) = cart_line.selling_plan else {
                return false;
            };
            self.store
                .variant_location(cart_line.variant_id)
                .is_some_and(|(product, _)| {
                    self.store
                        .selling_plan_of(&self.store.products[product], plan)
                        .is_some()
                })
        })
    }

    fn new_line(
        &mut self,
        merchandise_id: &str,
        quantity: u32,
        attributes: Vec<KeyValue>,
    ) -> Result<ResultLine, String> {
        let variant_id = gid_number(merchandise_id)
            .ok_or_else(|| format!("`{merchandise_id}` is not a product variant id"))?;
        let cart_line = CartLine {
            variant_id,
            quantity,
            properties: Default::default(),
            selling_plan: None,
        };
        let mut line = line_from_variant(self.store, &cart_line, attributes)
            .ok_or_else(|| format!("no product variant `{merchandise_id}`"))?;
        line.id = format!("gid://shopify/CartLine/{}", self.next_line_id);
        self.next_line_id += 1;
        Ok(line)
    }

    /// Shopify's pricing rules of `lineExpand`: either every component has a
    /// `fixedPricePerUnit` or none has, and an overall `percentageDecrease` cannot come with
    /// component prices. The bundle costs the sum of its components' prices when they have one,
    /// the price of the expanded line otherwise; the decrease applies to the latter. A bundle
    /// without component prices keeps its price, which is shared among its components in
    /// proportion to their variant prices.
    fn expand(&mut self, op: &Value) -> Result<(), String> {
        let id = str_field(op, "cartLineId").ok_or("expand without `cartLineId`")?;
        let index = self.transformable_line(id, "expand")?;
        let quantity = self.lines[index].quantity;

        let items: Vec<&Value> = array(op, "expandedCartItems").collect();
        if items.is_empty() {
            return Err("expand without `expandedCartItems`".to_string());
        }
        let fixed: Vec<Option<Cents>> = items
            .iter()
            .map(|item| {
                item.pointer("/price/adjustment/fixedPricePerUnit/amount")
                    .map(|amount| money_of(amount, "the `fixedPricePerUnit` of an expanded item"))
                    .transpose()
            })
            .collect::<Result<_, _>>()?;
        let priced = fixed.iter().filter(|price| price.is_some()).count();
        if priced != 0 && priced != items.len() {
            return Err(
                "expanded_items_missing_prices: one or more expanded cart items are missing prices, either all must have a price, or none"
                    .to_string(),
            );
        }
        let decrease = match op.pointer("/price/percentageDecrease/value") {
            Some(value) => Some(percentage_of(value).map_err(|error| {
                format!("invalid_price_adjustment_percentage_decrease: {error}")
            })?),
            None => None,
        };
        if priced != 0 && decrease.is_some() {
            return Err(
                "cannot_combine_price_adjustment_and_price_per_component: cannot combine both an overall price adjustment and individual prices for components"
                    .to_string(),
            );
        }

        let mut components = Vec::new();
        for (item, fixed) in items.iter().zip(&fixed) {
            let merchandise =
                str_field(item, "merchandiseId").ok_or("expanded item without `merchandiseId`")?;
            let per_unit = quantity_of(item.get("quantity"), "the quantity of an expanded item")?;
            let units = per_unit
                .checked_mul(quantity)
                .ok_or("an expanded item has a quantity that is too large")?;
            let attributes = attributes(item.get("attributes"));
            let mut component = self.new_line(merchandise, units, attributes)?;
            if let Some(amount) = fixed {
                component.unit_price = *amount;
            }
            // Whatever prices the component, its line has to fit (the shares below add them up).
            line_total(component.unit_price, units)?;
            components.push((component, per_unit));
        }

        let bundle_price = if priced != 0 {
            let mut sum: Cents = 0;
            for (component, per_unit) in &components {
                sum = sum
                    .checked_add(line_total(component.unit_price, *per_unit)?)
                    .ok_or(MONEY_OVERFLOW)?;
            }
            sum
        } else {
            let price = self.lines[index].unit_price;
            match decrease {
                Some(percentage) => (price as f64 * (1.0 - percentage / 100.0)).round() as Cents,
                None => price,
            }
        };
        let bundle_total = line_total(bundle_price, quantity)?;
        // Without prices of their own, the components share the price of the bundle, to the
        // cent: their line totals add up to the total of the expanded line.
        let shares = if priced != 0 {
            None
        } else {
            Some(allocate(bundle_total, &components))
        };
        for (position, (component, _)) in components.iter_mut().enumerate() {
            if let Some(shares) = &shares {
                let share = shares[position];
                // The unit price is the share per unit, to the cent. When the share does not
                // divide evenly, the line total stays exact and the unit price is rounded.
                let units = Cents::from(component.quantity.max(1));
                component.unit_price = (share as f64 / units as f64).round() as Cents;
                refresh(component);
                component.subtotal = share;
                component.total = share;
            } else {
                refresh(component);
            }
        }

        let line = &mut self.lines[index];
        line.unit_price = bundle_price;
        line.requires_shipping = components.iter().any(|(c, _)| c.requires_shipping);
        line.components = components
            .into_iter()
            .map(|(component, _)| component)
            .collect();
        line.transformed_by = Some("expand".to_string());
        override_presentation(line, op);
        Ok(())
    }

    fn merge(&mut self, op: &Value) -> Result<(), String> {
        let parent_variant =
            str_field(op, "parentVariantId").ok_or("merge without `parentVariantId`")?;
        // Resolve every source line before changing anything: a line is merged once, and no
        // more than it holds.
        let mut sources: Vec<(usize, u32)> = Vec::new();
        for source in array(op, "cartLines") {
            let id = str_field(source, "cartLineId").ok_or("merged line without `cartLineId`")?;
            let index = self.transformable_line(id, "merge")?;
            if sources.iter().any(|(seen, _)| *seen == index) {
                return Err(format!("the cart line `{id}` is merged more than once"));
            }
            let take = quantity_of(source.get("quantity"), "the quantity of a merged line")?;
            let available = self.lines[index].quantity;
            if take > available {
                return Err(format!(
                    "{take} of the cart line `{id}` are merged, which has {available}"
                ));
            }
            sources.push((index, take));
        }
        if sources.is_empty() {
            return Err("merge of no cart line".to_string());
        }

        let parent_id = gid_number(parent_variant)
            .ok_or_else(|| format!("`{parent_variant}` is not a product variant id"))?;
        let mut parent = self
            .new_line(parent_variant, 1, attributes(op.get("attributes")))
            .or_else(|_| {
                // A parent variant that is not in the store data still makes a line.
                let mut line = ResultLine {
                    id: format!("gid://shopify/CartLine/{}", self.next_line_id),
                    variant_id: parent_id,
                    title: "Bundle".to_string(),
                    quantity: 1,
                    attributes: attributes(op.get("attributes")),
                    ..ResultLine::default()
                };
                self.next_line_id += 1;
                line.requires_shipping = true;
                Ok::<_, String>(line)
            })?;

        // The price of the parent is checked before any line changes.
        let mut sum: Cents = 0;
        for (index, take) in &sources {
            let cost = line_total(self.lines[*index].unit_price, *take)?;
            sum = sum
                .checked_add(cost)
                .filter(|sum| *sum <= MAX_CENTS)
                .ok_or(MONEY_OVERFLOW)?;
        }
        let parent_unit_price = parent_price(sum, op.get("price"))?.unwrap_or(sum);

        let mut components = Vec::new();
        let mut cart_sources = Vec::new();
        for (index, take) in &sources {
            let line = &mut self.lines[*index];
            let mut component = line.clone();
            component.quantity = *take;
            refresh(&mut component);
            (component.sources, line.sources) = split_sources(&line.sources, *take, line.quantity);
            cart_sources.extend(component.sources.iter().copied());
            line.quantity -= take;
            if line.quantity > 0 {
                // A part of the line stays: the component is a new line.
                component.id = format!("gid://shopify/CartLine/{}", self.next_line_id);
                self.next_line_id += 1;
            }
            components.push(component);
        }
        parent.unit_price = parent_unit_price;
        parent.requires_shipping = components
            .iter()
            .any(|component| component.requires_shipping);
        parent.components = components;
        parent.sources = cart_sources;
        parent.transformed_by = Some("merge".to_string());
        override_presentation(&mut parent, op);
        refresh(&mut parent);

        let first = sources.iter().map(|(index, _)| *index).min().unwrap_or(0);
        let position = first.min(self.lines.len());
        // Lines nothing is left of go away; the parent takes the place of the first one.
        let mut lines = std::mem::take(&mut self.lines);
        let emptied: BTreeSet<usize> = sources
            .iter()
            .filter(|(index, _)| lines[*index].quantity == 0)
            .map(|(index, _)| *index)
            .collect();
        let mut result = Vec::with_capacity(lines.len());
        let mut parent = Some(parent);
        for (index, line) in lines.drain(..).enumerate() {
            if index == position
                && let Some(parent) = parent.take()
            {
                result.push(parent);
            }
            if !emptied.contains(&index) {
                result.push(line);
            }
        }
        result.extend(parent);
        self.lines = result;
        Ok(())
    }

    fn update(&mut self, op: &Value) -> Result<(), String> {
        let id = str_field(op, "cartLineId").ok_or("update without `cartLineId`")?;
        let index = self.transformable_line(id, "update")?;
        let amount = op
            .pointer("/price/adjustment/fixedPricePerUnit/amount")
            .map(|amount| money_of(amount, "the `fixedPricePerUnit` of an update"))
            .transpose()?;
        let line = &mut self.lines[index];
        if let Some(amount) = amount {
            line_total(amount, line.quantity)?;
            line.unit_price = amount;
        }
        override_presentation(line, op);
        if line.transformed_by.is_none() {
            line.transformed_by = Some("update".to_string());
        }
        Ok(())
    }

    // ---- discounts ----

    /// The discounts a Function backs that are active now: automatic ones, and those whose code
    /// the buyer entered.
    fn active_discounts(&self) -> Vec<(usize, &'a DiscountConfig, Option<String>)> {
        let config = self.config;
        let state = self.state;
        config
            .discounts
            .iter()
            .enumerate()
            .filter(|(_, discount)| self.enabled.contains(&discount.function))
            .filter_map(|(index, discount)| match &discount.code {
                None => Some((index, discount, None)),
                Some(code) => state
                    .discount_codes
                    .iter()
                    .find(|entered| entered.eq_ignore_ascii_case(code))
                    .map(|_| (index, discount, Some(code.clone()))),
            })
            .collect()
    }

    fn discount_function(&mut self, discount: &DiscountConfig) -> Option<&'a FunctionExtension> {
        let app = self.app;
        match app.function(&discount.function) {
            Some(function) if function.version.is_none() => {
                self.errors.push(CheckoutError {
                    message: format!(
                        "discount \"{}\": the Function `{}` is not run, its api_version is not supported (supported: {})",
                        discount.title,
                        discount.function,
                        ApiVersion::supported()
                    ),
                    target: None,
                    function: Some(function.handle.clone()),
                });
                None
            }
            Some(function) => Some(function),
            None => {
                self.errors.push(CheckoutError {
                    message: format!(
                        "discount \"{}\": the app has no Function `{}`",
                        discount.title, discount.function
                    ),
                    target: None,
                    function: None,
                });
                None
            }
        }
    }

    fn line_discounts(&mut self) {
        for (index, discount, code) in self.active_discounts() {
            let Some(function) = self.discount_function(discount) else {
                continue;
            };
            let classes = classes_of(discount);
            if !classes.contains(&DiscountClass::Product)
                && !classes.contains(&DiscountClass::Order)
            {
                continue;
            }
            let targets = self.targets_of(
                function,
                &[
                    TargetKind::LinesDiscounts,
                    TargetKind::ProductDiscount,
                    TargetKind::OrderDiscount,
                ],
            );
            for target in targets {
                let legacy = target.kind != TargetKind::LinesDiscounts;
                if target.kind == TargetKind::ProductDiscount
                    && !classes.contains(&DiscountClass::Product)
                    || target.kind == TargetKind::OrderDiscount
                        && !classes.contains(&DiscountClass::Order)
                {
                    continue;
                }
                let (kind, node) = if legacy {
                    (OwnerKind::DiscountNode, "DiscountNode")
                } else if code.is_some() {
                    (OwnerKind::Discount, "DiscountCodeNode")
                } else {
                    (OwnerKind::Discount, "DiscountAutomaticNode")
                };
                let owner = OwnerSpec {
                    kind,
                    id: format!("gid://shopify/{node}/{}", index + 1),
                    metafields: &discount.metafields,
                    classes: &classes,
                };
                let Some(output) = self.execute(function, target, owner, code.as_deref()) else {
                    continue;
                };
                self.apply_line_discounts(
                    function,
                    discount,
                    code.as_deref(),
                    target.kind,
                    &classes,
                    &output,
                );
            }
        }
    }

    fn apply_line_discounts(
        &mut self,
        function: &FunctionExtension,
        discount: &DiscountConfig,
        code: Option<&str>,
        kind: TargetKind,
        classes: &[DiscountClass],
        output: &Value,
    ) {
        let context = DiscountContext {
            function: &function.handle,
            title: &discount.title,
            code,
        };
        match kind {
            TargetKind::LinesDiscounts => {
                for operation in array(output, "operations") {
                    if let Some(op) = operation.get("productDiscountsAdd") {
                        if classes.contains(&DiscountClass::Product) {
                            let strategy = str_field(op, "selectionStrategy");
                            match self.product_candidates(op, "candidates") {
                                Ok(candidates) => {
                                    self.apply_product(&context, strategy, candidates)
                                }
                                Err(error) => self
                                    .function_error(function, format!("product discount: {error}")),
                            }
                        }
                    } else if let Some(op) = operation.get("orderDiscountsAdd") {
                        if classes.contains(&DiscountClass::Order) {
                            let strategy = str_field(op, "selectionStrategy");
                            match candidates_of(op, "candidates") {
                                Ok(candidates) => self.apply_order(&context, strategy, candidates),
                                Err(error) => self
                                    .function_error(function, format!("order discount: {error}")),
                            }
                        }
                    } else if let Some(op) = operation.get("enteredDiscountCodesAccept") {
                        for entry in array(op, "codes") {
                            if let Some(code) = str_field(entry, "code") {
                                self.accepted_codes.push(code.to_lowercase());
                            }
                        }
                    } else if let Some(op) = operation.get("enteredDiscountCodesReject") {
                        for entry in array(op, "codes") {
                            if let Some(code) = str_field(entry, "code") {
                                self.rejected_codes.push(code.to_lowercase());
                            }
                        }
                    }
                }
            }
            TargetKind::ProductDiscount => {
                let strategy = str_field(output, "discountApplicationStrategy");
                match self.product_candidates(output, "discounts") {
                    Ok(candidates) => self.apply_product(&context, strategy, candidates),
                    Err(error) => {
                        self.function_error(function, format!("product discount: {error}"))
                    }
                }
            }
            TargetKind::OrderDiscount => {
                let strategy = str_field(output, "discountApplicationStrategy");
                match candidates_of(output, "discounts") {
                    Ok(candidates) => self.apply_order(&context, strategy, candidates),
                    Err(error) => self.function_error(function, format!("order discount: {error}")),
                }
            }
            _ => {}
        }
    }

    /// The candidates of a product discount: valid ones that only target cart lines that exist.
    fn product_candidates(&self, op: &Value, key: &str) -> Result<Vec<Candidate>, String> {
        let candidates = candidates_of(op, key)?;
        for target in candidates.iter().flat_map(|candidate| &candidate.targets) {
            if let Target::Line { id, .. } = target
                && self.lines.iter().all(|line| &line.id != id)
            {
                return Err(format!("no cart line `{id}`"));
            }
        }
        Ok(candidates)
    }

    fn apply_product(
        &mut self,
        context: &DiscountContext,
        strategy: Option<&str>,
        candidates: Vec<Candidate>,
    ) {
        let chosen = select(strategy, candidates, |candidate| {
            plan_product(&self.lines, candidate)
                .iter()
                .map(|(_, cents)| cents)
                .sum()
        });
        for candidate in chosen {
            let plan = plan_product(&self.lines, &candidate);
            self.allocate(context, &candidate, DiscountClass::Product, plan);
        }
    }

    fn apply_order(
        &mut self,
        context: &DiscountContext,
        strategy: Option<&str>,
        candidates: Vec<Candidate>,
    ) {
        let chosen = select(strategy, candidates, |candidate| {
            plan_order(&self.lines, candidate)
                .iter()
                .map(|(_, cents)| cents)
                .sum()
        });
        for candidate in chosen {
            let plan = plan_order(&self.lines, &candidate);
            self.allocate(context, &candidate, DiscountClass::Order, plan);
        }
    }

    fn allocate(
        &mut self,
        context: &DiscountContext,
        candidate: &Candidate,
        class: DiscountClass,
        plan: Vec<(usize, Cents)>,
    ) {
        let title = candidate
            .message
            .clone()
            .unwrap_or_else(|| context.title.to_string());
        let code = candidate
            .code
            .clone()
            .or_else(|| context.code.map(str::to_string));
        let mut total = 0;
        for (index, cents) in plan {
            let line = &mut self.lines[index];
            let cents = cents.min(line.total);
            if cents <= 0 {
                continue;
            }
            line.total -= cents;
            line.discount_allocations.push(DiscountAllocation {
                title: title.clone(),
                code: code.clone(),
                amount: cents,
                class,
            });
            total += cents;
        }
        if total > 0 {
            self.discounts.push(AppliedDiscount {
                title,
                code,
                class,
                amount: total,
                function: context.function.to_string(),
            });
        }
    }

    // ---- delivery ----

    fn build_delivery_groups(&mut self) {
        let line_ids: Vec<String> = self
            .lines
            .iter()
            .filter(|line| line.requires_shipping)
            .map(|line| line.id.clone())
            .collect();
        if line_ids.is_empty() || self.config.shipping_rates.is_empty() {
            return;
        }
        let options = self
            .config
            .shipping_rates
            .iter()
            .map(|rate| DeliveryOption {
                handle: rate.handle.clone(),
                title: rate.title.clone(),
                description: rate.description.clone(),
                kind: rate.kind,
                original_cost: rate.price,
                cost: rate.price,
            })
            .collect();
        self.groups = vec![DeliveryGroup {
            handle: DELIVERY_GROUP_HANDLE.to_string(),
            line_ids,
            options,
            selected: None,
        }];
        self.select_delivery();
    }

    /// The buyer's choice if it is still offered, otherwise the first option.
    fn select_delivery(&mut self) {
        for group in &mut self.groups {
            let chosen = self
                .state
                .selected_delivery
                .get(&group.handle)
                .filter(|handle| group.options.iter().any(|option| &option.handle == *handle))
                .cloned();
            group.selected =
                chosen.or_else(|| group.options.first().map(|option| option.handle.clone()));
        }
    }

    fn delivery_customizations(&mut self) {
        for (index, function) in self.functions_with(&[TargetKind::DeliveryCustomization]) {
            for target in self.targets_of(function, &[TargetKind::DeliveryCustomization]) {
                let owner = OwnerSpec {
                    kind: OwnerKind::DeliveryCustomization,
                    id: format!("gid://shopify/DeliveryCustomization/{}", index + 1),
                    metafields: self.function_metafields(&function.handle),
                    classes: &[],
                };
                if let Some(output) = self.execute(function, target, owner, None) {
                    for operation in array(&output, "operations") {
                        if let Err(message) = self.apply_delivery_operation(operation) {
                            self.function_error(
                                function,
                                format!("delivery customization: {message}"),
                            );
                        }
                    }
                    self.select_delivery();
                }
            }
        }
    }

    /// An operation of `cart.delivery-options.transform.run`: `deliveryOptionHide`,
    /// `deliveryOptionMove`, `deliveryOptionRename`. An option that is not offered is left alone.
    fn apply_delivery_operation(&mut self, operation: &Value) -> Result<(), String> {
        if let Some(op) = operation.get("deliveryOptionHide") {
            let handle = handle_of(op, "deliveryOptionHandle", "deliveryOptionHide")?;
            for group in &mut self.groups {
                group.options.retain(|option| option.handle != handle);
            }
        } else if let Some(op) = operation.get("deliveryOptionMove") {
            let handle = handle_of(op, "deliveryOptionHandle", "deliveryOptionMove")?;
            let to = index_of(op.get("index"), "the `index` of a deliveryOptionMove")?;
            for group in &mut self.groups {
                if let Some(from) = group
                    .options
                    .iter()
                    .position(|option| option.handle == handle)
                {
                    let option = group.options.remove(from);
                    group.options.insert(to.min(group.options.len()), option);
                }
            }
        } else if let Some(op) = operation.get("deliveryOptionRename") {
            let handle = handle_of(op, "deliveryOptionHandle", "deliveryOptionRename")?;
            let title = handle_of(op, "title", "deliveryOptionRename")?;
            for group in &mut self.groups {
                if let Some(option) = group
                    .options
                    .iter_mut()
                    .find(|option| option.handle == handle)
                {
                    option.title = title.to_string();
                }
            }
        }
        Ok(())
    }

    fn shipping_discounts(&mut self) {
        for (index, discount, code) in self.active_discounts() {
            let Some(function) = self.discount_function(discount) else {
                continue;
            };
            if !classes_of(discount).contains(&DiscountClass::Shipping) {
                continue;
            }
            let classes = classes_of(discount);
            for target in self.targets_of(
                function,
                &[TargetKind::DeliveryDiscounts, TargetKind::ShippingDiscount],
            ) {
                let legacy = target.kind == TargetKind::ShippingDiscount;
                let (kind, node) = if legacy {
                    (OwnerKind::DiscountNode, "DiscountNode")
                } else if code.is_some() {
                    (OwnerKind::Discount, "DiscountCodeNode")
                } else {
                    (OwnerKind::Discount, "DiscountAutomaticNode")
                };
                let owner = OwnerSpec {
                    kind,
                    id: format!("gid://shopify/{node}/{}", index + 1),
                    metafields: &discount.metafields,
                    classes: &classes,
                };
                let Some(output) = self.execute(function, target, owner, code.as_deref()) else {
                    continue;
                };
                let context = DiscountContext {
                    function: &function.handle,
                    title: &discount.title,
                    code: code.as_deref(),
                };
                if legacy {
                    let strategy = str_field(&output, "discountApplicationStrategy");
                    match candidates_of(&output, "discounts") {
                        Ok(candidates) => self.apply_shipping(&context, strategy, candidates),
                        Err(error) => {
                            self.function_error(function, format!("shipping discount: {error}"))
                        }
                    }
                } else {
                    for operation in array(&output, "operations") {
                        if let Some(op) = operation.get("deliveryDiscountsAdd") {
                            let strategy = str_field(op, "selectionStrategy");
                            match candidates_of(op, "candidates") {
                                Ok(candidates) => {
                                    self.apply_shipping(&context, strategy, candidates)
                                }
                                Err(error) => self.function_error(
                                    function,
                                    format!("shipping discount: {error}"),
                                ),
                            }
                        } else if let Some(op) = operation.get("enteredDiscountCodesAccept") {
                            for entry in array(op, "codes") {
                                if let Some(code) = str_field(entry, "code") {
                                    self.accepted_codes.push(code.to_lowercase());
                                }
                            }
                        } else if let Some(op) = operation.get("enteredDiscountCodesReject") {
                            for entry in array(op, "codes") {
                                if let Some(code) = str_field(entry, "code") {
                                    self.rejected_codes.push(code.to_lowercase());
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    fn apply_shipping(
        &mut self,
        context: &DiscountContext,
        strategy: Option<&str>,
        candidates: Vec<Candidate>,
    ) {
        let chosen = select(strategy, candidates, |candidate| {
            let mut groups = self.groups.clone();
            reduce_shipping(&mut groups, candidate)
        });
        for candidate in chosen {
            let reduced = reduce_shipping(&mut self.groups, &candidate);
            if reduced > 0 {
                self.discounts.push(AppliedDiscount {
                    title: candidate
                        .message
                        .clone()
                        .unwrap_or_else(|| context.title.to_string()),
                    code: candidate
                        .code
                        .clone()
                        .or_else(|| context.code.map(str::to_string)),
                    class: DiscountClass::Shipping,
                    amount: reduced,
                    function: context.function.to_string(),
                });
            }
        }
    }

    // ---- payment, validation ----

    fn payment_customizations(&mut self) {
        for (index, function) in self.functions_with(&[TargetKind::PaymentCustomization]) {
            for target in self.targets_of(function, &[TargetKind::PaymentCustomization]) {
                let owner = OwnerSpec {
                    kind: OwnerKind::PaymentCustomization,
                    id: format!("gid://shopify/PaymentCustomization/{}", index + 1),
                    metafields: self.function_metafields(&function.handle),
                    classes: &[],
                };
                if let Some(output) = self.execute(function, target, owner, None) {
                    for operation in array(&output, "operations") {
                        if let Err(message) = self.apply_payment_operation(operation) {
                            self.function_error(
                                function,
                                format!("payment customization: {message}"),
                            );
                        }
                    }
                }
            }
        }
    }

    /// An operation of `cart.payment-methods.transform.run`: `paymentMethodHide`,
    /// `paymentMethodMove`, `paymentMethodRename`. Payment terms and order reviews
    /// (`paymentTermsSet`, `orderReviewAdd`) have no place in the local checkout.
    fn apply_payment_operation(&mut self, operation: &Value) -> Result<(), String> {
        let position = |methods: &[PaymentMethod], id: &str| {
            methods
                .iter()
                .position(|method| id_matches(id, &method.handle))
        };
        if let Some(op) = operation.get("paymentMethodHide") {
            let id = handle_of(op, "paymentMethodId", "paymentMethodHide")?;
            // Hidden from the accelerated checkout only: the method stays in the list.
            let in_list = op
                .get("placements")
                .and_then(Value::as_array)
                .is_none_or(|placements| {
                    placements.is_empty()
                        || placements
                            .iter()
                            .any(|placement| placement.as_str() == Some("PAYMENT_METHOD"))
                });
            if in_list && let Some(at) = position(&self.payment_methods, id) {
                self.payment_methods.remove(at);
            }
        } else if let Some(op) = operation.get("paymentMethodMove") {
            let id = handle_of(op, "paymentMethodId", "paymentMethodMove")?;
            let to = index_of(op.get("index"), "the `index` of a paymentMethodMove")?;
            if let Some(from) = position(&self.payment_methods, id) {
                let method = self.payment_methods.remove(from);
                self.payment_methods
                    .insert(to.min(self.payment_methods.len()), method);
            }
        } else if let Some(op) = operation.get("paymentMethodRename") {
            let id = handle_of(op, "paymentMethodId", "paymentMethodRename")?;
            let name = handle_of(op, "name", "paymentMethodRename")?;
            if let Some(at) = position(&self.payment_methods, id) {
                self.payment_methods[at].name = name.to_string();
            }
        }
        Ok(())
    }

    fn validations(&mut self) {
        for (index, function) in self.functions_with(&[TargetKind::Validation]) {
            for target in self.targets_of(function, &[TargetKind::Validation]) {
                let owner = OwnerSpec {
                    kind: OwnerKind::Validation,
                    id: format!("gid://shopify/Validation/{}", index + 1),
                    metafields: self.function_metafields(&function.handle),
                    classes: &[],
                };
                let Some(output) = self.execute(function, target, owner, None) else {
                    continue;
                };
                // `purchase.validation.run` returns `{ errors }`, `cart.validations.generate.run`
                // returns `{ operations: [{ validationAdd: { errors } }] }`.
                let legacy = array(&output, "errors");
                let added = array(&output, "operations")
                    .filter_map(|operation| operation.get("validationAdd"))
                    .flat_map(|add| array(add, "errors"));
                let errors: Vec<CheckoutError> = legacy
                    .chain(added)
                    .map(|error| CheckoutError {
                        message: str_field(error, "message")
                            .or_else(|| str_field(error, "localizedMessage"))
                            .unwrap_or("The checkout is not valid")
                            .to_string(),
                        target: str_field(error, "target").map(str::to_string),
                        function: Some(function.handle.clone()),
                    })
                    .collect();
                self.errors.extend(errors);
            }
        }
    }

    // ---- cost ----

    fn finish(self) -> CheckoutResult {
        let subtotal: Cents = self.lines.iter().map(|line| line.subtotal).sum();
        let discounts: Cents = self
            .discounts
            .iter()
            .filter(|discount| discount.class != DiscountClass::Shipping)
            .map(|discount| discount.amount)
            .sum();
        let shipping: Cents = self.groups.iter().map(DeliveryGroup::selected_cost).sum();
        let taxable = (subtotal - discounts).max(0);
        let tax = (taxable as f64 * self.config.tax_rate_bps as f64 / 10_000.0).round() as Cents;

        let discount_codes = self
            .state
            .discount_codes
            .iter()
            .map(|code| {
                let lower = code.to_lowercase();
                let applied = self.discounts.iter().any(|discount| {
                    discount
                        .code
                        .as_ref()
                        .is_some_and(|c| c.eq_ignore_ascii_case(code))
                });
                let accepted =
                    self.accepted_codes.contains(&lower) && !self.rejected_codes.contains(&lower);
                DiscountCodeStatus {
                    code: code.clone(),
                    applicable: applied || accepted,
                }
            })
            .collect();

        CheckoutResult {
            lines: self.lines,
            discounts: self.discounts,
            discount_codes,
            delivery_groups: self.groups,
            payment_methods: self.payment_methods,
            errors: self.errors,
            cost: Cost {
                subtotal,
                discounts,
                shipping,
                tax,
                total: subtotal - discounts + shipping + tax,
                currency_code: self.currency,
            },
            runs: self.runs,
        }
    }
}

struct DiscountContext<'a> {
    function: &'a str,
    title: &'a str,
    code: Option<&'a str>,
}

fn classes_of(discount: &DiscountConfig) -> Vec<DiscountClass> {
    if discount.classes.is_empty() {
        vec![
            DiscountClass::Product,
            DiscountClass::Order,
            DiscountClass::Shipping,
        ]
    } else {
        discount.classes.clone()
    }
}

/// The variables of the input query: the JSON value of the owner's metafield that
/// `[extensions.input.variables]` names.
fn owner_variables(
    function: &FunctionExtension,
    metafields: &[Metafield],
) -> Result<Option<Value>, String> {
    let Some(variables) = &function.input_variables else {
        return Ok(None);
    };
    let Some(metafield) = metafields.iter().find(|metafield| {
        metafield.namespace == variables.namespace && metafield.key == variables.key
    }) else {
        return Ok(None);
    };
    serde_json::from_str(&metafield.value).map(Some).map_err(|error| {
        format!(
            "the metafield {}.{} is not JSON ({error}), it cannot give the input query its variables",
            variables.namespace, variables.key
        )
    })
}

// ---- lines ----

fn initial_lines(store: &lsf_core::Store, session: &lsf_core::Session) -> Vec<ResultLine> {
    session
        .cart_lines
        .iter()
        .enumerate()
        .filter_map(|(index, cart_line)| {
            let attributes = cart_line
                .properties
                .iter()
                .map(|(key, value)| KeyValue {
                    key: key.clone(),
                    value: value.clone(),
                })
                .collect();
            let mut line = line_from_variant(store, cart_line, attributes)?;
            line.id = format!("gid://shopify/CartLine/{}", index + 1);
            line.sources = vec![SourceLine {
                index,
                quantity: line.quantity,
            }];
            Some(line)
        })
        .collect()
}

/// A checkout line for a cart line, priced as the storefront cart prices it: with the selling
/// plan of the line and the quantity price breaks of the variant ([`ResolvedLine`]).
fn line_from_variant(
    store: &lsf_core::Store,
    cart_line: &CartLine,
    attributes: Vec<KeyValue>,
) -> Option<ResultLine> {
    let variant_id = cart_line.variant_id;
    let (product_index, variant_index) = store.variant_location(variant_id)?;
    let product = &store.products[product_index];
    let variant = &product.variants[variant_index];
    let resolved = ResolvedLine {
        line: cart_line,
        product_index,
        variant_index,
        product,
        variant,
        selling_plan: cart_line
            .selling_plan
            .and_then(|id| store.selling_plan_of(product, id)),
    };
    let quantity = cart_line.quantity;
    let image = variant
        .media_index
        .and_then(|index| product.media.get(index))
        .and_then(|media| media.preview.as_ref())
        .or_else(|| product.images().next())
        .map(|image| {
            format!(
                "/cdn/shop/files/{}",
                lsf_core::urls::encode_path(&image.src)
            )
        });
    let mut line = ResultLine {
        id: String::new(),
        variant_id,
        product_id: product.id,
        title: product.title.clone(),
        variant_title: (variant.title != "Default Title").then(|| variant.title.clone()),
        sku: (!variant.sku.is_empty()).then(|| variant.sku.clone()),
        image,
        quantity,
        unit_price: resolved.unit_price(),
        attributes,
        requires_shipping: variant.requires_shipping,
        ..ResultLine::default()
    };
    refresh(&mut line);
    Some(line)
}

/// Divides the cart lines a line is made of when `take` of its `quantity` units go elsewhere:
/// what goes, and what stays.
fn split_sources(
    sources: &[SourceLine],
    take: u32,
    quantity: u32,
) -> (Vec<SourceLine>, Vec<SourceLine>) {
    let (mut taken, mut left) = (Vec::new(), Vec::new());
    for source in sources {
        let part = if quantity == 0 {
            0
        } else {
            (u64::from(source.quantity) * u64::from(take) / u64::from(quantity)) as u32
        };
        for (list, amount) in [(&mut taken, part), (&mut left, source.quantity - part)] {
            if amount > 0 {
                list.push(SourceLine {
                    index: source.index,
                    quantity: amount,
                });
            }
        }
    }
    (taken, left)
}

/// Recomputes the amounts of a line from its unit price, quantity and allocations.
fn refresh(line: &mut ResultLine) {
    line.subtotal = line.unit_price.saturating_mul(Cents::from(line.quantity));
    let allocated: Cents = line.discount_allocations.iter().map(|a| a.amount).sum();
    line.total = (line.subtotal - allocated).max(0);
}

fn override_presentation(line: &mut ResultLine, op: &Value) {
    if let Some(title) = str_field(op, "title") {
        line.title = title.to_string();
    }
    if let Some(url) = op.pointer("/image/url").and_then(Value::as_str) {
        line.image = Some(url.to_string());
    }
}

/// Shares `total` (the price of every bundle of an expanded line) among its components in
/// proportion to their variant prices (a component counts as many times as it has units in the
/// bundle). The shares are exact: they add up to `total`, to the cent, even when a share does
/// not divide evenly by the units of its component.
fn allocate(total: Cents, components: &[(ResultLine, u32)]) -> Vec<Cents> {
    let weights: Vec<Cents> = components
        .iter()
        .map(|(component, per_unit)| component.unit_price.saturating_mul(Cents::from(*per_unit)))
        .collect();
    if weights.iter().sum::<Cents>() > 0 {
        return distribute(total, &weights);
    }
    // Components without a price share it by units.
    let units: Vec<Cents> = components
        .iter()
        .map(|(_, per_unit)| Cents::from(*per_unit))
        .collect();
    distribute(total, &units)
}

/// The price of a bundle line after a price adjustment of the operation, per unit. `None` when
/// the operation has no price adjustment.
fn parent_price(sum: Cents, price: Option<&Value>) -> Result<Option<Cents>, String> {
    let Some(price) = price else {
        return Ok(None);
    };
    if let Some(percentage) = price.pointer("/percentageDecrease/value") {
        let percentage = percentage_of(percentage)
            .map_err(|error| format!("invalid_price_adjustment_percentage_decrease: {error}"))?;
        return Ok(Some(
            (sum as f64 * (1.0 - percentage / 100.0)).round() as Cents
        ));
    }
    price
        .pointer("/adjustment/fixedPricePerUnit/amount")
        .or_else(|| price.pointer("/fixedPricePerUnit/amount"))
        .map(|amount| money_of(amount, "the `fixedPricePerUnit` of a merge"))
        .transpose()
}

fn attributes(value: Option<&Value>) -> Vec<KeyValue> {
    value
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|attribute| {
            Some(KeyValue {
                key: str_field(attribute, "key")?.to_string(),
                value: str_field(attribute, "value")?.to_string(),
            })
        })
        .collect()
}

// ---- discount candidates ----

#[derive(Clone, Debug)]
struct Candidate {
    targets: Vec<Target>,
    message: Option<String>,
    /// The code the candidate says it is for (`associatedDiscountCode`).
    code: Option<String>,
    value: Amount,
}

#[derive(Clone, Debug)]
enum Target {
    Line {
        id: String,
        quantity: Option<u32>,
    },
    Variant {
        id: u64,
        quantity: Option<u32>,
    },
    Order {
        excluded_lines: Vec<String>,
        excluded_variants: Vec<u64>,
    },
    DeliveryGroup(String),
    DeliveryOption(String),
}

#[derive(Clone, Copy, Debug)]
enum Amount {
    Percentage(f64),
    Fixed { cents: Cents, each: bool },
    None,
}

/// A target of a discount candidate: a cart line, a product variant, the order, a delivery group
/// or option. Ids and quantities have to be valid, a target is never dropped.
fn target_of(target: &Value) -> Result<Target, String> {
    if let Some(line) = target.get("cartLine") {
        Ok(Target::Line {
            id: handle_of(line, "id", "a cartLine target")?.to_string(),
            quantity: optional_quantity(line.get("quantity"), "the quantity of a cartLine target")?,
        })
    } else if let Some(variant) = target.get("productVariant") {
        Ok(Target::Variant {
            id: variant_id_of(variant.get("id"), "a productVariant target")?,
            quantity: optional_quantity(
                variant.get("quantity"),
                "the quantity of a productVariant target",
            )?,
        })
    } else if let Some(order) = target.get("orderSubtotal") {
        let excluded_lines = array(order, "excludedCartLineIds")
            .map(|id| {
                id.as_str()
                    .map(str::to_string)
                    .ok_or_else(|| format!("an excluded cart line id is not a string: {id}"))
            })
            .collect::<Result<_, _>>()?;
        let excluded_variants = array(order, "excludedVariantIds")
            .map(|id| variant_id_of(Some(id), "an excluded variant"))
            .collect::<Result<_, _>>()?;
        Ok(Target::Order {
            excluded_lines,
            excluded_variants,
        })
    } else if let Some(group) = target.get("deliveryGroup") {
        Ok(Target::DeliveryGroup(
            handle_of(group, "id", "a deliveryGroup target")?.to_string(),
        ))
    } else if let Some(option) = target.get("deliveryOption") {
        Ok(Target::DeliveryOption(
            handle_of(option, "handle", "a deliveryOption target")?.to_string(),
        ))
    } else {
        Err(format!("a discount target of an unknown kind: {target}"))
    }
}

/// Reads a discount candidate of the current output shape (`productDiscountsAdd` candidates:
/// `fixedAmount.amount`) or the legacy one (`discounts`: `fixedAmount.value`).
fn candidate(raw: &Value) -> Result<Candidate, String> {
    let targets = array(raw, "targets")
        .map(target_of)
        .collect::<Result<Vec<_>, _>>()?;

    // New shape: `value: { percentage: { value }, fixedAmount: { amount, appliesToEachItem } }`.
    // Legacy: `fixedAmount: { value, appliesToEachItem }`.
    let value = match raw.get("value") {
        Some(value) => {
            if let Some(percentage) = value.pointer("/percentage/value") {
                Amount::Percentage(
                    percentage_of(percentage)
                        .map_err(|error| format!("the percentage of a discount: {error}"))?,
                )
            } else if let Some(fixed) = value.get("fixedAmount") {
                let amount = fixed
                    .get("amount")
                    .or(fixed.get("value"))
                    .ok_or("a fixed amount discount without an amount")?;
                Amount::Fixed {
                    cents: money_of(amount, "the fixed amount of a discount")?,
                    each: fixed
                        .get("appliesToEachItem")
                        .and_then(Value::as_bool)
                        .unwrap_or(false),
                }
            } else {
                Amount::None
            }
        }
        None => Amount::None,
    };

    Ok(Candidate {
        targets,
        message: str_field(raw, "message").map(str::to_string),
        code: raw
            .pointer("/associatedDiscountCode/code")
            .and_then(Value::as_str)
            .map(str::to_string),
        value,
    })
}

/// Which candidates a selection strategy applies: all of them, the first one, or the one that
/// takes the most off (`amount_of` measures a candidate).
fn select(
    strategy: Option<&str>,
    candidates: Vec<Candidate>,
    amount_of: impl Fn(&Candidate) -> Cents,
) -> Vec<Candidate> {
    match strategy {
        Some("ALL") => candidates,
        Some("MAXIMUM") => candidates
            .into_iter()
            .max_by_key(|candidate| amount_of(candidate))
            .into_iter()
            .collect(),
        // FIRST, and a strategy the output does not name.
        _ => candidates.into_iter().take(1).collect(),
    }
}

/// The lines a `productVariant` target covers, `(line index, quantity)`. The quantity of the
/// target is one budget for the variant, spent line after line (two lines of a variant with
/// different properties are two lines of the same budget); without one, every line is covered
/// whole.
fn variant_coverage(lines: &[ResultLine], id: u64, quantity: Option<u32>) -> Vec<(usize, u32)> {
    let mut budget = quantity;
    let mut covered = Vec::new();
    for (index, line) in lines.iter().enumerate() {
        if line.variant_id != id {
            continue;
        }
        let take = budget.map_or(line.quantity, |left| left.min(line.quantity));
        if let Some(left) = &mut budget {
            *left -= take;
        }
        if take > 0 {
            covered.push((index, take));
        }
    }
    covered
}

/// What a product candidate takes off each line it targets: `(line index, cents)`.
fn plan_product(lines: &[ResultLine], candidate: &Candidate) -> Vec<(usize, Cents)> {
    // (line, quantity covered)
    let mut covered: Vec<(usize, u32)> = Vec::new();
    for target in &candidate.targets {
        let matching: Vec<(usize, Option<u32>)> = match target {
            Target::Line { id, quantity } => lines
                .iter()
                .position(|line| &line.id == id)
                .map(|index| (index, *quantity))
                .into_iter()
                .collect(),
            Target::Variant { id, quantity } => variant_coverage(lines, *id, *quantity)
                .into_iter()
                .map(|(index, covered)| (index, Some(covered)))
                .collect(),
            _ => Vec::new(),
        };
        for (index, quantity) in matching {
            let quantity = quantity.map_or(lines[index].quantity, |q| q.min(lines[index].quantity));
            match covered.iter_mut().find(|(line, _)| *line == index) {
                Some(entry) => entry.1 = entry.1.max(quantity),
                None => covered.push((index, quantity)),
            }
        }
    }
    let bases: Vec<Cents> = covered
        .iter()
        .map(|(index, quantity)| {
            lines[*index]
                .unit_price
                .saturating_mul(Cents::from(*quantity))
        })
        .collect();
    let amounts: Vec<Cents> = match candidate.value {
        Amount::Percentage(percentage) => bases
            .iter()
            .map(|base| (*base as f64 * percentage / 100.0).round() as Cents)
            .collect(),
        Amount::Fixed { cents, each: true } => covered
            .iter()
            .map(|(_, quantity)| cents.saturating_mul(Cents::from(*quantity)))
            .collect(),
        Amount::Fixed { cents, each: false } => distribute(cents.min(bases.iter().sum()), &bases),
        Amount::None => vec![0; covered.len()],
    };
    covered
        .iter()
        .zip(amounts)
        .map(|((index, _), cents)| (*index, cents.min(lines[*index].total)))
        .collect()
}

/// What an order candidate takes off each line: the amount is shared between the lines it
/// covers in proportion to what they cost after the line discounts. An `orderSubtotal` target
/// covers every line but the excluded ones; a `productVariant` target covers the lines of the
/// variant, for as many items as its quantity says (one budget for the variant), at what those
/// items cost after the discounts.
fn plan_order(lines: &[ResultLine], candidate: &Candidate) -> Vec<(usize, Cents)> {
    // (line index, what the candidate covers of it)
    let mut covered: Vec<(usize, Cents)> = Vec::new();
    let mut cover = |index: usize, base: Cents| match covered.iter_mut().find(|(i, _)| *i == index)
    {
        Some(entry) => entry.1 = entry.1.max(base),
        None => covered.push((index, base)),
    };
    for target in &candidate.targets {
        match target {
            Target::Order {
                excluded_lines,
                excluded_variants,
            } => {
                for (index, line) in lines.iter().enumerate() {
                    if !excluded_lines.contains(&line.id)
                        && !excluded_variants.contains(&line.variant_id)
                    {
                        cover(index, line.total);
                    }
                }
            }
            Target::Variant { id, quantity } => {
                for (index, take) in variant_coverage(lines, *id, *quantity) {
                    let line = &lines[index];
                    let base = i128::from(line.total) * i128::from(take)
                        / i128::from(line.quantity.max(1));
                    cover(index, Cents::try_from(base).unwrap_or(line.total));
                }
            }
            _ => {}
        }
    }
    covered.sort_by_key(|(index, _)| *index);
    let weights: Vec<Cents> = covered.iter().map(|(_, base)| *base).collect();
    let base: Cents = weights.iter().sum();
    let amount = match candidate.value {
        Amount::Percentage(percentage) => (base as f64 * percentage / 100.0).round() as Cents,
        Amount::Fixed { cents, .. } => cents.min(base),
        Amount::None => 0,
    };
    covered
        .into_iter()
        .zip(distribute(amount, &weights))
        .map(|((index, _), cents)| (index, cents))
        .collect()
}

/// Shares `total` between weights in proportion, to the cent (largest remainders first).
fn distribute(total: Cents, weights: &[Cents]) -> Vec<Cents> {
    let sum: Cents = weights.iter().sum();
    if sum <= 0 || total <= 0 {
        return vec![0; weights.len()];
    }
    let mut shares: Vec<Cents> = weights
        .iter()
        .map(|weight| (i128::from(total) * i128::from(*weight) / i128::from(sum)) as Cents)
        .collect();
    let mut left = total - shares.iter().sum::<Cents>();
    let mut order: Vec<usize> = (0..weights.len()).collect();
    order.sort_by_key(|index| {
        let remainder = (i128::from(total) * i128::from(weights[*index])) % i128::from(sum);
        std::cmp::Reverse(remainder)
    });
    for index in order {
        if left == 0 {
            break;
        }
        shares[index] += 1;
        left -= 1;
    }
    shares
}

/// Lowers the cost of the options a shipping candidate targets. Returns what the options the
/// buyer selected got cheaper by.
fn reduce_shipping(groups: &mut [DeliveryGroup], candidate: &Candidate) -> Cents {
    let mut reduced = 0;
    for group in groups.iter_mut() {
        let group_targeted = candidate.targets.iter().any(|target| match target {
            Target::DeliveryGroup(id) => id_matches(id, &group.handle),
            _ => false,
        });
        for option in &mut group.options {
            let targeted = group_targeted
                || candidate
                    .targets
                    .iter()
                    .any(|target| matches!(target, Target::DeliveryOption(handle) if handle == &option.handle));
            if !targeted {
                continue;
            }
            let cut = match candidate.value {
                Amount::Percentage(percentage) => {
                    (option.cost as f64 * percentage / 100.0).round() as Cents
                }
                Amount::Fixed { cents, .. } => cents,
                Amount::None => 0,
            }
            .clamp(0, option.cost);
            option.cost -= cut;
            if group.selected.as_ref() == Some(&option.handle) {
                reduced += cut;
            }
        }
    }
    reduced
}

// ---- JSON helpers ----

fn array<'v>(value: &'v Value, key: &str) -> impl Iterator<Item = &'v Value> {
    value
        .get(key)
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
}

fn str_field<'v>(value: &'v Value, key: &str) -> Option<&'v str> {
    value.get(key).and_then(Value::as_str)
}

/// A decimal, which Functions write as a string (`"19.99"`) or a number.
fn number_of(value: &Value) -> Option<f64> {
    match value {
        Value::String(text) => text.trim().parse().ok(),
        other => other.as_f64(),
    }
}

/// The largest amount, in cents, a Function can give, and the largest a line can cost (10 billion
/// in the currency of the shop): sums of such amounts cannot overflow.
const MAX_CENTS: Cents = 1_000_000_000_000;

const MONEY_OVERFLOW: &str = "the amount is too large";

/// A monetary value of a Function output (`what` says which, for the error): a decimal that is
/// finite, not negative and not too large. Shopify rejects the output of a Function otherwise.
fn money_of(value: &Value, what: &str) -> Result<Cents, String> {
    let amount = number_of(value).ok_or_else(|| format!("{what} is not a number: {value}"))?;
    if !amount.is_finite() {
        return Err(format!("{what} is not finite: {value}"));
    }
    if amount < 0.0 {
        return Err(format!("{what} is negative: {value}"));
    }
    if amount * 100.0 > MAX_CENTS as f64 {
        return Err(format!("{what} is too large: {value}"));
    }
    Ok((amount * 100.0).round() as Cents)
}

/// A percentage of a Function output: between 0 and 100.
fn percentage_of(value: &Value) -> Result<f64, String> {
    let percentage = number_of(value).ok_or_else(|| format!("{value} is not a number"))?;
    if !(0.0..=100.0).contains(&percentage) {
        return Err(format!(
            "the percentage must be between 0 and 100, not {value}"
        ));
    }
    Ok(percentage)
}

/// What `quantity` units at `unit_price` cost, if a line can cost that.
fn line_total(unit_price: Cents, quantity: u32) -> Result<Cents, String> {
    unit_price
        .checked_mul(Cents::from(quantity))
        .filter(|total| *total <= MAX_CENTS)
        .ok_or_else(|| MONEY_OVERFLOW.to_string())
}

/// The largest quantity a Function can give: a GraphQL `Int`.
const MAX_QUANTITY: u64 = i32::MAX as u64;

/// A quantity of a Function output (`what` says which, for the error): a whole number from 1 to
/// the largest `Int`. A quantity is never corrected to fit.
fn quantity_of(value: Option<&Value>, what: &str) -> Result<u32, String> {
    let value = value.ok_or_else(|| format!("{what} is missing"))?;
    value
        .as_u64()
        .filter(|quantity| (1..=MAX_QUANTITY).contains(quantity))
        .and_then(|quantity| u32::try_from(quantity).ok())
        .ok_or_else(|| format!("{what} is not a whole number from 1 to {MAX_QUANTITY}: {value}"))
}

/// [`quantity_of`] for a quantity the output may leave out (or give as `null`).
fn optional_quantity(value: Option<&Value>, what: &str) -> Result<Option<u32>, String> {
    match value {
        None | Some(Value::Null) => Ok(None),
        some => quantity_of(some, what).map(Some),
    }
}

/// A position in a list of a Function output: a whole number from 0 to the largest `Int`.
fn index_of(value: Option<&Value>, what: &str) -> Result<usize, String> {
    let value = value.ok_or_else(|| format!("{what} is missing"))?;
    value
        .as_u64()
        .filter(|index| *index <= MAX_QUANTITY)
        .and_then(|index| usize::try_from(index).ok())
        .ok_or_else(|| format!("{what} is not a whole number from 0 to {MAX_QUANTITY}: {value}"))
}

/// The string `key` of an operation or target (an id, a handle, a title), which has to be there.
fn handle_of<'v>(value: &'v Value, key: &str, what: &str) -> Result<&'v str, String> {
    str_field(value, key).ok_or_else(|| format!("{what} without `{key}`"))
}

/// The id of a product variant a Function names: a gid that ends with its number.
fn variant_id_of(value: Option<&Value>, what: &str) -> Result<u64, String> {
    value
        .and_then(Value::as_str)
        .and_then(gid_number)
        .ok_or_else(|| format!("{what} has no valid product variant id: {value:?}"))
}

/// The candidates of a discount operation (`candidates`) or of a legacy output (`discounts`).
/// One candidate that is not valid rejects the whole list.
fn candidates_of(op: &Value, key: &str) -> Result<Vec<Candidate>, String> {
    array(op, key).map(candidate).collect()
}

/// The number at the end of a gid, `gid://shopify/ProductVariant/42?x=1` → 42.
fn gid_number(gid: &str) -> Option<u64> {
    gid.split('?').next()?.rsplit('/').next()?.parse().ok()
}

/// Whether an id a Function gives (a handle, or a gid ending with it) names `handle`.
fn id_matches(id: &str, handle: &str) -> bool {
    id == handle || id.rsplit('/').next() == Some(handle)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn line(id: u64, variant: u64, price: Cents, quantity: u32) -> ResultLine {
        let mut line = ResultLine {
            id: format!("gid://shopify/CartLine/{id}"),
            variant_id: variant,
            unit_price: price,
            quantity,
            requires_shipping: true,
            ..ResultLine::default()
        };
        refresh(&mut line);
        line
    }

    fn product_candidate(raw: Value) -> Candidate {
        candidate(&raw).unwrap()
    }

    #[test]
    fn a_percentage_applies_to_each_targeted_line() {
        let lines = vec![line(1, 10, 1000, 2), line(2, 11, 500, 1)];
        let candidate = product_candidate(json!({
            "targets": [{"cartLine": {"id": "gid://shopify/CartLine/1"}}],
            "value": {"percentage": {"value": "10.0"}},
        }));
        assert_eq!(plan_product(&lines, &candidate), vec![(0, 200)]);
    }

    #[test]
    fn a_fixed_amount_is_per_item_or_shared() {
        let lines = vec![line(1, 10, 1000, 2), line(2, 11, 3000, 1)];
        let targets = json!([
            {"cartLine": {"id": "gid://shopify/CartLine/1"}},
            {"cartLine": {"id": "gid://shopify/CartLine/2"}},
        ]);
        let each = product_candidate(json!({
            "targets": targets, "value": {"fixedAmount": {"amount": "1.00", "appliesToEachItem": true}},
        }));
        assert_eq!(plan_product(&lines, &each), vec![(0, 200), (1, 100)]);
        let shared = product_candidate(json!({
            "targets": targets, "value": {"fixedAmount": {"amount": "10.00"}},
        }));
        // 10.00 shared by 20.00 and 30.00: 4.00 and 6.00.
        assert_eq!(plan_product(&lines, &shared), vec![(0, 400), (1, 600)]);
    }

    #[test]
    fn a_fixed_amount_never_exceeds_the_line() {
        let lines = vec![line(1, 10, 300, 1)];
        let candidate = product_candidate(json!({
            "targets": [{"cartLine": {"id": "gid://shopify/CartLine/1"}}],
            "value": {"fixedAmount": {"amount": "5.00"}},
        }));
        assert_eq!(plan_product(&lines, &candidate), vec![(0, 300)]);
    }

    #[test]
    fn the_legacy_shape_targets_variants() {
        let lines = vec![line(1, 42, 1000, 3), line(2, 43, 1000, 1)];
        let candidate = candidate(&json!({
            "message": "Ten off",
            "targets": [{"productVariant": {"id": "gid://shopify/ProductVariant/42", "quantity": 2}}],
            "value": {"fixedAmount": {"value": "2.00", "appliesToEachItem": true}},
        }))
        .unwrap();
        assert_eq!(candidate.message.as_deref(), Some("Ten off"));
        assert_eq!(plan_product(&lines, &candidate), vec![(0, 400)]);
    }

    #[test]
    fn a_variant_quantity_is_one_budget_for_all_the_lines_of_the_variant() {
        // Two lines of the same variant, 32.00 each, told apart by their properties.
        let lines = vec![
            line(1, 42, 3200, 1),
            line(2, 42, 3200, 1),
            line(3, 43, 500, 1),
        ];
        let target = |quantity: Value| {
            product_candidate(json!({
                "targets": [{"productVariant": {"id": "gid://shopify/ProductVariant/42", "quantity": quantity}}],
                "value": {"percentage": {"value": "10"}},
            }))
        };
        assert_eq!(plan_product(&lines, &target(json!(1))), vec![(0, 320)]);
        assert_eq!(
            plan_product(&lines, &target(json!(2))),
            vec![(0, 320), (1, 320)]
        );
        assert_eq!(
            plan_product(&lines, &target(json!(9))),
            vec![(0, 320), (1, 320)]
        );
        // Each target has its own budget.
        let two = product_candidate(json!({
            "targets": [
                {"productVariant": {"id": "gid://shopify/ProductVariant/42", "quantity": 1}},
                {"productVariant": {"id": "gid://shopify/ProductVariant/42", "quantity": 1}},
            ],
            "value": {"percentage": {"value": "10"}},
        }));
        assert_eq!(plan_product(&lines, &two), vec![(0, 320)]);
    }

    #[test]
    fn a_variant_budget_spans_the_lines_in_order() {
        let lines = vec![line(1, 42, 1000, 2), line(2, 42, 1000, 3)];
        let candidate = product_candidate(json!({
            "targets": [{"productVariant": {"id": "gid://shopify/ProductVariant/42", "quantity": 3}}],
            "value": {"fixedAmount": {"amount": "1.00", "appliesToEachItem": true}},
        }));
        // 2 items of the first line, 1 of the second.
        assert_eq!(plan_product(&lines, &candidate), vec![(0, 200), (1, 100)]);
    }

    #[test]
    fn an_order_discount_can_target_variants_and_their_quantity() {
        let lines = vec![
            line(1, 42, 3200, 1),
            line(2, 42, 3200, 3),
            line(3, 43, 1000, 1),
        ];
        let target = |quantity: Option<u32>| {
            let mut variant = json!({"id": "gid://shopify/ProductVariant/42"});
            if let Some(quantity) = quantity {
                variant["quantity"] = json!(quantity);
            }
            product_candidate(json!({
                "targets": [{"productVariant": variant}],
                "value": {"percentage": {"value": "10"}},
            }))
        };
        // All the items of the variant: 10 % of 4 x 32.00, shared 1:3. Not the other variant.
        assert_eq!(plan_order(&lines, &target(None)), vec![(0, 320), (1, 960)]);
        // Two items in all: the line of one, then one of the other. One budget, not one per line.
        assert_eq!(
            plan_order(&lines, &target(Some(2))),
            vec![(0, 320), (1, 320)]
        );
        // A fixed amount is shared between the items, up to what they cost.
        let fixed = product_candidate(json!({
            "targets": [{"productVariant": {"id": "gid://shopify/ProductVariant/42", "quantity": 2}}],
            "value": {"fixedAmount": {"value": "10.00"}},
        }));
        assert_eq!(plan_order(&lines, &fixed), vec![(0, 500), (1, 500)]);
        // A variant the cart does not hold covers nothing.
        let none = product_candidate(json!({
            "targets": [{"productVariant": {"id": "gid://shopify/ProductVariant/99"}}],
            "value": {"percentage": {"value": "10"}},
        }));
        assert!(plan_order(&lines, &none).is_empty());
    }

    #[test]
    fn a_variant_target_of_an_order_discount_costs_what_the_discounts_left() {
        let mut lines = vec![line(1, 42, 3200, 2)];
        // A line discount took 14.00 off: the two items cost 50.00.
        lines[0].total = 5000;
        let whole = product_candidate(json!({
            "targets": [{"productVariant": {"id": "gid://shopify/ProductVariant/42"}}],
            "value": {"percentage": {"value": "10"}},
        }));
        assert_eq!(plan_order(&lines, &whole), vec![(0, 500)]);
        // One item of the two is half of what is left.
        let one = product_candidate(json!({
            "targets": [{"productVariant": {"id": "gid://shopify/ProductVariant/42", "quantity": 1}}],
            "value": {"percentage": {"value": "10"}},
        }));
        assert_eq!(plan_order(&lines, &one), vec![(0, 250)]);
        // A fixed amount never takes more than that.
        let fixed = product_candidate(json!({
            "targets": [{"productVariant": {"id": "gid://shopify/ProductVariant/42", "quantity": 1}}],
            "value": {"fixedAmount": {"value": "100.00"}},
        }));
        assert_eq!(plan_order(&lines, &fixed), vec![(0, 2500)]);
    }

    #[test]
    fn an_order_discount_is_shared_between_lines_and_skips_excluded_ones() {
        let lines = vec![
            line(1, 10, 1000, 1),
            line(2, 11, 3000, 1),
            line(3, 12, 5000, 1),
        ];
        let candidate = product_candidate(json!({
            "targets": [{"orderSubtotal": {"excludedCartLineIds": ["gid://shopify/CartLine/3"]}}],
            "value": {"percentage": {"value": 10}},
        }));
        // 10 % of 40.00, shared 1:3.
        assert_eq!(plan_order(&lines, &candidate), vec![(0, 100), (1, 300)]);
    }

    #[test]
    fn distribution_adds_up_to_the_cent() {
        assert_eq!(distribute(100, &[1, 1, 1]), vec![34, 33, 33]);
        assert_eq!(distribute(5, &[0, 0]), vec![0, 0]);
        assert_eq!(distribute(1000, &[1000, 3000]).iter().sum::<i64>(), 1000);
    }

    #[test]
    fn strategies_pick_candidates() {
        let candidates = vec![
            product_candidate(
                json!({"message": "small", "value": {"fixedAmount": {"amount": "1"}}}),
            ),
            product_candidate(json!({"message": "big", "value": {"fixedAmount": {"amount": "9"}}})),
            product_candidate(json!({"message": "mid", "value": {"fixedAmount": {"amount": "5"}}})),
        ];
        let amount = |candidate: &Candidate| match candidate.value {
            Amount::Fixed { cents, .. } => cents,
            _ => 0,
        };
        let names = |chosen: Vec<Candidate>| -> Vec<String> {
            chosen.into_iter().filter_map(|c| c.message).collect()
        };
        assert_eq!(
            names(select(Some("FIRST"), candidates.clone(), amount)),
            ["small"]
        );
        assert_eq!(
            names(select(Some("MAXIMUM"), candidates.clone(), amount)),
            ["big"]
        );
        assert_eq!(names(select(Some("ALL"), candidates, amount)).len(), 3);
    }

    #[test]
    fn shipping_discounts_cut_the_selected_option() {
        let option = |handle: &str, cost| DeliveryOption {
            handle: handle.into(),
            title: handle.into(),
            description: None,
            kind: crate::DeliveryKind::Shipping,
            original_cost: cost,
            cost,
        };
        let mut groups = vec![DeliveryGroup {
            handle: DELIVERY_GROUP_HANDLE.into(),
            line_ids: vec![],
            options: vec![option("standard", 490), option("express", 1290)],
            selected: Some("standard".into()),
        }];
        let candidate = product_candidate(json!({
            "targets": [{"deliveryGroup": {"id": "gid://shopify/CartDeliveryGroup/group-1"}}],
            "value": {"percentage": {"value": "100"}},
        }));
        assert_eq!(reduce_shipping(&mut groups, &candidate), 490);
        assert_eq!(groups[0].options[0].cost, 0);
        assert_eq!(groups[0].options[1].cost, 0);
        assert_eq!(groups[0].options[1].original_cost, 1290);
    }

    #[test]
    fn ids_match_handles_and_gids() {
        assert!(id_matches("card", "card"));
        assert!(id_matches(
            "gid://shopify/PaymentCustomizationPaymentMethod/card",
            "card"
        ));
        assert!(!id_matches(
            "gid://shopify/PaymentCustomizationPaymentMethod/cards",
            "card"
        ));
        assert_eq!(gid_number("gid://shopify/ProductVariant/42?a=1"), Some(42));
        assert_eq!(gid_number("nope"), None);
    }

    #[test]
    fn decimals_are_strings_or_numbers() {
        let money = |value: Value| money_of(&value, "an amount");
        assert_eq!(money(json!("19.99")), Ok(1999));
        assert_eq!(money(json!(19.995)), Ok(2000));
        assert_eq!(money(json!(5)), Ok(500));
        assert!(money(json!(null)).is_err());
    }

    #[test]
    fn money_must_be_finite_positive_and_not_overflow() {
        let money = |value: Value| money_of(&value, "an amount");
        assert_eq!(money(json!("0")), Ok(0));
        for bad in [
            json!("-10.00"),
            json!(-0.01),
            json!("NaN"),
            json!("inf"),
            json!("-inf"),
            json!("1e300"),
            json!(1e18),
            json!("12,5"),
            json!(true),
            json!([]),
        ] {
            assert!(money(bad.clone()).is_err(), "{bad} was accepted");
        }
        assert!(percentage_of(&json!("100")).is_ok());
        for bad in [
            json!(-1),
            json!("100.01"),
            json!("NaN"),
            json!("inf"),
            json!({}),
        ] {
            assert!(percentage_of(&bad).is_err(), "{bad} was accepted");
        }
        assert!(line_total(MAX_CENTS, 2).is_err());
        assert!(line_total(Cents::MAX, 2).is_err());
        assert_eq!(line_total(1999, 3), Ok(5997));
    }

    /// The candidates of every discount output kind: the current shapes (product, order,
    /// delivery) and the legacy ones.
    #[test]
    fn a_discount_candidate_with_invalid_money_is_rejected() {
        let target = json!([{"cartLine": {"id": "gid://shopify/CartLine/1"}}]);
        let values = [
            json!({"fixedAmount": {"amount": "-5.00"}}),
            json!({"fixedAmount": {"value": "-5.00"}}),
            json!({"fixedAmount": {"amount": "NaN"}}),
            json!({"fixedAmount": {"amount": "1e300"}}),
            json!({"fixedAmount": {"amount": "abc"}}),
            json!({"fixedAmount": {}}),
            json!({"percentage": {"value": "-10"}}),
            json!({"percentage": {"value": "150"}}),
            json!({"percentage": {"value": "inf"}}),
        ];
        for value in values {
            // `productDiscountsAdd`, `orderDiscountsAdd` and `deliveryDiscountsAdd` carry
            // `candidates`, the legacy outputs `discounts`.
            for key in ["candidates", "discounts"] {
                let op = json!({ key: [
                    {"targets": target, "value": {"percentage": {"value": "10"}}},
                    {"targets": target, "value": value},
                ]});
                assert!(
                    candidates_of(&op, key).is_err(),
                    "{value} was accepted in `{key}`"
                );
            }
        }
        let good = json!({"candidates": [
            {"targets": target, "value": {"percentage": {"value": "100"}}},
            {"targets": target, "value": {"fixedAmount": {"amount": "0"}}},
        ]});
        assert_eq!(candidates_of(&good, "candidates").unwrap().len(), 2);
    }

    #[test]
    fn quantities_and_indexes_are_never_corrected() {
        let quantity = |value: Value| quantity_of(Some(&value), "a quantity");
        assert_eq!(quantity(json!(1)), Ok(1));
        assert_eq!(quantity(json!(2147483647)), Ok(2147483647));
        for bad in [
            json!(0),
            json!(-2),
            json!(1.5),
            json!(2.0),
            json!("3"),
            json!(null),
            json!(2147483648u64),
            json!(4294967297u64),
            json!(u64::MAX),
            json!(i64::MIN),
        ] {
            assert!(quantity(bad.clone()).is_err(), "{bad} was accepted");
        }
        assert!(quantity_of(None, "a quantity").is_err());
        assert_eq!(optional_quantity(None, "a quantity"), Ok(None));
        assert_eq!(
            optional_quantity(Some(&json!(null)), "a quantity"),
            Ok(None)
        );
        assert!(optional_quantity(Some(&json!(0)), "a quantity").is_err());

        let index = |value: Value| index_of(Some(&value), "an index");
        assert_eq!(index(json!(0)), Ok(0));
        assert_eq!(index(json!(2147483647)), Ok(2147483647));
        for bad in [
            json!(-1),
            json!(0.5),
            json!("1"),
            json!(2147483648u64),
            json!(u64::MAX),
        ] {
            assert!(index(bad.clone()).is_err(), "{bad} was accepted");
        }
    }

    #[test]
    fn a_target_is_never_dropped_for_an_invalid_id_or_quantity() {
        for target in [
            json!({"productVariant": {"id": "nope"}}),
            json!({"productVariant": {"id": "gid://shopify/ProductVariant/x1"}}),
            json!({"productVariant": {}}),
            json!({"productVariant": {"id": "gid://shopify/ProductVariant/1", "quantity": 0}}),
            json!({"productVariant": {"id": "gid://shopify/ProductVariant/1", "quantity": 4294967297u64}}),
            json!({"cartLine": {"id": "gid://shopify/CartLine/1", "quantity": -1}}),
            json!({"cartLine": {}}),
            json!({"orderSubtotal": {"excludedVariantIds": ["gid://shopify/ProductVariant/1", "x"]}}),
            json!({"orderSubtotal": {"excludedCartLineIds": [1]}}),
            json!({"deliveryGroup": {}}),
            json!({"deliveryOption": {}}),
            json!({"somethingElse": {}}),
        ] {
            let raw = json!({"targets": [target], "value": {"percentage": {"value": "10"}}});
            assert!(candidate(&raw).is_err(), "{target} was accepted");
        }
        let raw = json!({"targets": [
            {"productVariant": {"id": "gid://shopify/ProductVariant/1", "quantity": null}},
            {"orderSubtotal": {"excludedVariantIds": ["gid://shopify/ProductVariant/2"]}},
        ]});
        assert_eq!(candidate(&raw).unwrap().targets.len(), 2);
    }

    #[test]
    fn a_huge_fixed_discount_per_item_does_not_overflow() {
        let lines = vec![line(1, 10, 1000, u32::MAX)];
        let candidate = product_candidate(json!({
            "targets": [{"cartLine": {"id": "gid://shopify/CartLine/1"}}],
            "value": {"fixedAmount": {"amount": "9999999999.99", "appliesToEachItem": true}},
        }));
        assert_eq!(plan_product(&lines, &candidate), vec![(0, lines[0].total)]);
    }

    fn component(variant: u64, price: Cents, quantity: u32, per_unit: u32) -> (ResultLine, u32) {
        (line(variant, variant, price, quantity * per_unit), per_unit)
    }

    #[test]
    fn components_share_the_price_of_the_bundles_to_the_cent() {
        // Two 32.00 bundles of 3 x 22.00 and 1 x 24.00: 64.00 in all.
        let components = [component(1, 2200, 2, 3), component(2, 2400, 2, 1)];
        let shares = allocate(6400, &components);
        assert_eq!(shares, vec![4693, 1707]);
        assert_eq!(shares.iter().sum::<Cents>(), 6400);
    }

    #[test]
    fn shares_add_up_whatever_the_prices() {
        for (total, prices) in [
            (1, vec![(1, 1), (1, 1), (1, 1)]),
            (10_001, vec![(333, 3), (777, 1), (1, 7)]),
            (999, vec![(0, 2), (0, 5)]),
            (0, vec![(100, 1)]),
        ] {
            let components: Vec<_> = prices
                .iter()
                .enumerate()
                .map(|(i, (price, per_unit))| component(i as u64 + 1, *price, 1, *per_unit))
                .collect();
            let shares = allocate(total, &components);
            assert_eq!(shares.iter().sum::<Cents>(), total, "{prices:?}");
            assert!(shares.iter().all(|share| *share >= 0));
        }
    }
}
