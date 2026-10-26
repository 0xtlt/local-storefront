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

use std::collections::BTreeSet;

use chrono::{DateTime, FixedOffset, Utc};
use serde_json::Value;

use crate::app::{App, FunctionExtension, FunctionTarget, Selection, TargetKind};
use crate::input::{InputContext, Owner, OwnerKind};
use crate::runner;
use crate::{
    AppliedDiscount, Cents, CheckoutConfig, CheckoutError, CheckoutResult, CheckoutState, Cost,
    DeliveryGroup, DeliveryOption, DiscountAllocation, DiscountClass, DiscountCodeStatus,
    DiscountConfig, FunctionRun, KeyValue, Metafield, PaymentMethod, ResultLine,
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

        let execution = runner::run(&function.wasm_path, &target.export, &input);
        let failed = execution.error.is_some() || execution.output.is_none();
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
        if execution.instructions > runner::INSTRUCTIONS_LIMIT {
            self.errors.push(CheckoutError {
                message: format!(
                    "Function `{}` used {} instructions: Shopify stops a Function past {}",
                    function.handle,
                    execution.instructions,
                    runner::INSTRUCTIONS_LIMIT
                ),
                target: None,
                function: Some(function.handle.clone()),
            });
        }
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
        if failed { None } else { execution.output }
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

    fn function_error(&mut self, function: &FunctionExtension, message: String) {
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

    fn new_line(
        &mut self,
        merchandise_id: &str,
        quantity: u32,
        attributes: Vec<KeyValue>,
    ) -> Result<ResultLine, String> {
        let variant_id = gid_number(merchandise_id)
            .ok_or_else(|| format!("`{merchandise_id}` is not a product variant id"))?;
        let mut line = line_from_variant(self.store, variant_id, quantity, attributes)
            .ok_or_else(|| format!("no product variant `{merchandise_id}`"))?;
        line.id = format!("gid://shopify/CartLine/{}", self.next_line_id);
        self.next_line_id += 1;
        Ok(line)
    }

    fn expand(&mut self, op: &Value) -> Result<(), String> {
        let id = str_field(op, "cartLineId").ok_or("expand without `cartLineId`")?;
        let index = self.line_index(id)?;
        let quantity = self.lines[index].quantity;
        let mut components = Vec::new();
        for item in array(op, "expandedCartItems") {
            let merchandise =
                str_field(item, "merchandiseId").ok_or("expanded item without `merchandiseId`")?;
            let per_unit = item.get("quantity").and_then(Value::as_u64).unwrap_or(1) as u32;
            let attributes = attributes(item.get("attributes"));
            let mut component = self.new_line(merchandise, per_unit * quantity, attributes)?;
            if let Some(amount) = item
                .pointer("/price/adjustment/fixedPricePerUnit/amount")
                .and_then(cents_of)
            {
                component.unit_price = amount;
            }
            refresh(&mut component);
            components.push((component, per_unit));
        }
        let per_parent_unit: Cents = components
            .iter()
            .map(|(component, per_unit)| component.unit_price * Cents::from(*per_unit))
            .sum();
        let line = &mut self.lines[index];
        line.unit_price = parent_price(per_parent_unit, op.get("price")).unwrap_or(per_parent_unit);
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
        // Resolve every source line before changing anything.
        let mut sources = Vec::new();
        for source in array(op, "cartLines") {
            let id = str_field(source, "cartLineId").ok_or("merged line without `cartLineId`")?;
            let index = self.line_index(id)?;
            let take = source
                .get("quantity")
                .and_then(Value::as_u64)
                .map_or(self.lines[index].quantity, |q| q as u32)
                .min(self.lines[index].quantity);
            sources.push((index, take));
        }
        if sources.is_empty() {
            return Err("merge of no cart line".to_string());
        }

        let mut parent = self
            .new_line(parent_variant, 1, attributes(op.get("attributes")))
            .or_else(|_| {
                // A parent variant that is not in the store data still makes a line.
                let mut line = ResultLine {
                    id: format!("gid://shopify/CartLine/{}", self.next_line_id),
                    variant_id: gid_number(parent_variant).unwrap_or(0),
                    title: "Bundle".to_string(),
                    quantity: 1,
                    attributes: attributes(op.get("attributes")),
                    ..ResultLine::default()
                };
                self.next_line_id += 1;
                line.requires_shipping = true;
                Ok::<_, String>(line)
            })?;

        let mut components = Vec::new();
        for (index, take) in &sources {
            let line = &mut self.lines[*index];
            let mut component = line.clone();
            component.quantity = *take;
            refresh(&mut component);
            line.quantity -= take;
            if line.quantity > 0 {
                // A part of the line stays: the component is a new line.
                component.id = format!("gid://shopify/CartLine/{}", self.next_line_id);
                self.next_line_id += 1;
            }
            components.push(component);
        }
        let sum: Cents = components.iter().map(|component| component.subtotal).sum();
        parent.unit_price = parent_price(sum, op.get("price")).unwrap_or(sum);
        parent.requires_shipping = components
            .iter()
            .any(|component| component.requires_shipping);
        parent.components = components;
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
        let index = self.line_index(id)?;
        let line = &mut self.lines[index];
        if let Some(amount) = op
            .pointer("/price/adjustment/fixedPricePerUnit/amount")
            .and_then(cents_of)
        {
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
                            let candidates = array(op, "candidates").map(candidate).collect();
                            self.apply_product(&context, strategy, candidates);
                        }
                    } else if let Some(op) = operation.get("orderDiscountsAdd") {
                        if classes.contains(&DiscountClass::Order) {
                            let strategy = str_field(op, "selectionStrategy");
                            let candidates = array(op, "candidates").map(candidate).collect();
                            self.apply_order(&context, strategy, candidates);
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
                let candidates = array(output, "discounts").map(candidate).collect();
                self.apply_product(&context, strategy, candidates);
            }
            TargetKind::OrderDiscount => {
                let strategy = str_field(output, "discountApplicationStrategy");
                let candidates = array(output, "discounts").map(candidate).collect();
                self.apply_order(&context, strategy, candidates);
            }
            _ => {}
        }
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
                        self.apply_delivery_operation(operation);
                    }
                    self.select_delivery();
                }
            }
        }
    }

    fn apply_delivery_operation(&mut self, operation: &Value) {
        for group in &mut self.groups {
            if let Some(op) = operation.get("hide") {
                if let Some(handle) = str_field(op, "deliveryOptionHandle") {
                    group.options.retain(|option| option.handle != handle);
                }
            } else if let Some(op) = operation.get("move") {
                let (Some(handle), Some(to)) = (
                    str_field(op, "deliveryOptionHandle"),
                    op.get("index").and_then(Value::as_u64),
                ) else {
                    continue;
                };
                if let Some(from) = group
                    .options
                    .iter()
                    .position(|option| option.handle == handle)
                {
                    let option = group.options.remove(from);
                    group
                        .options
                        .insert((to as usize).min(group.options.len()), option);
                }
            } else if let Some(op) = operation.get("rename")
                && let Some(handle) = str_field(op, "deliveryOptionHandle")
                && let Some(option) = group
                    .options
                    .iter_mut()
                    .find(|option| option.handle == handle)
            {
                if let Some(title) = str_field(op, "title") {
                    option.title = title.to_string();
                }
                if let Some(description) = str_field(op, "description") {
                    option.description = Some(description.to_string());
                }
            }
        }
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
                    let candidates = array(&output, "discounts").map(candidate).collect();
                    self.apply_shipping(&context, strategy, candidates);
                } else {
                    for operation in array(&output, "operations") {
                        if let Some(op) = operation.get("deliveryDiscountsAdd") {
                            let strategy = str_field(op, "selectionStrategy");
                            let candidates = array(op, "candidates").map(candidate).collect();
                            self.apply_shipping(&context, strategy, candidates);
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
                        self.apply_payment_operation(operation);
                    }
                }
            }
        }
    }

    fn apply_payment_operation(&mut self, operation: &Value) {
        let position = |methods: &[PaymentMethod], op: &Value| {
            let id = str_field(op, "paymentMethodId")?;
            methods
                .iter()
                .position(|method| id_matches(id, &method.handle))
        };
        if let Some(op) = operation.get("hide") {
            if let Some(at) = position(&self.payment_methods, op) {
                self.payment_methods.remove(at);
            }
        } else if let Some(op) = operation.get("move") {
            if let (Some(from), Some(to)) = (
                position(&self.payment_methods, op),
                op.get("index").and_then(Value::as_u64),
            ) {
                let method = self.payment_methods.remove(from);
                let to = (to as usize).min(self.payment_methods.len());
                self.payment_methods.insert(to, method);
            }
        } else if let Some(op) = operation.get("rename")
            && let Some(at) = position(&self.payment_methods, op)
            && let Some(name) = str_field(op, "name")
        {
            self.payment_methods[at].name = name.to_string();
        }
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
        let shipping: Cents = self
            .groups
            .iter()
            .filter_map(|group| {
                let selected = group.selected.as_ref()?;
                group
                    .options
                    .iter()
                    .find(|option| &option.handle == selected)
            })
            .map(|option| option.cost)
            .sum();
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
            let mut line =
                line_from_variant(store, cart_line.variant_id, cart_line.quantity, attributes)?;
            line.id = format!("gid://shopify/CartLine/{}", index + 1);
            Some(line)
        })
        .collect()
}

fn line_from_variant(
    store: &lsf_core::Store,
    variant_id: u64,
    quantity: u32,
    attributes: Vec<KeyValue>,
) -> Option<ResultLine> {
    let (product, variant) = store.variant(variant_id)?;
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
        unit_price: variant.price,
        attributes,
        requires_shipping: variant.requires_shipping,
        ..ResultLine::default()
    };
    refresh(&mut line);
    Some(line)
}

/// Recomputes the amounts of a line from its unit price, quantity and allocations.
fn refresh(line: &mut ResultLine) {
    line.subtotal = line.unit_price * Cents::from(line.quantity);
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

/// The price of a bundle line after a price adjustment of the operation, per unit.
fn parent_price(sum: Cents, price: Option<&Value>) -> Option<Cents> {
    let price = price?;
    if let Some(percentage) = price
        .pointer("/percentageDecrease/value")
        .and_then(number_of)
    {
        return Some((sum as f64 * (1.0 - percentage / 100.0)).round() as Cents);
    }
    price
        .pointer("/adjustment/fixedPricePerUnit/amount")
        .or_else(|| price.pointer("/fixedPricePerUnit/amount"))
        .and_then(cents_of)
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

/// Reads a discount candidate of the current output shape (`productDiscountsAdd` candidates:
/// `fixedAmount.amount`) or the legacy one (`discounts`: `fixedAmount.value`).
fn candidate(raw: &Value) -> Candidate {
    let targets = array(raw, "targets")
        .filter_map(|target| {
            if let Some(line) = target.get("cartLine") {
                Some(Target::Line {
                    id: str_field(line, "id")?.to_string(),
                    quantity: line
                        .get("quantity")
                        .and_then(Value::as_u64)
                        .map(|q| q as u32),
                })
            } else if let Some(variant) = target.get("productVariant") {
                Some(Target::Variant {
                    id: gid_number(str_field(variant, "id")?)?,
                    quantity: variant
                        .get("quantity")
                        .and_then(Value::as_u64)
                        .map(|q| q as u32),
                })
            } else if let Some(order) = target.get("orderSubtotal") {
                Some(Target::Order {
                    excluded_lines: array(order, "excludedCartLineIds")
                        .filter_map(|id| id.as_str().map(str::to_string))
                        .collect(),
                    excluded_variants: array(order, "excludedVariantIds")
                        .filter_map(|id| id.as_str().and_then(gid_number))
                        .collect(),
                })
            } else if let Some(group) = target.get("deliveryGroup") {
                Some(Target::DeliveryGroup(str_field(group, "id")?.to_string()))
            } else if let Some(option) = target.get("deliveryOption") {
                Some(Target::DeliveryOption(
                    str_field(option, "handle")?.to_string(),
                ))
            } else {
                None
            }
        })
        .collect();

    // New shape: `value: { percentage: { value }, fixedAmount: { amount, appliesToEachItem } }`.
    // Legacy: `fixedAmount: { value, appliesToEachItem }`.
    let value = raw.get("value");
    let amount = value
        .and_then(|value| value.pointer("/percentage/value"))
        .and_then(number_of)
        .map(Amount::Percentage)
        .or_else(|| {
            let fixed = value?.get("fixedAmount")?;
            Some(Amount::Fixed {
                cents: fixed
                    .get("amount")
                    .or(fixed.get("value"))
                    .and_then(cents_of)?,
                each: fixed
                    .get("appliesToEachItem")
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
            })
        })
        .unwrap_or(Amount::None);

    Candidate {
        targets,
        message: str_field(raw, "message").map(str::to_string),
        code: raw
            .pointer("/associatedDiscountCode/code")
            .and_then(Value::as_str)
            .map(str::to_string),
        value: amount,
    }
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
            Target::Variant { id, quantity } => lines
                .iter()
                .enumerate()
                .filter(|(_, line)| line.variant_id == *id)
                .map(|(index, _)| (index, *quantity))
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
        .map(|(index, quantity)| lines[*index].unit_price * Cents::from(*quantity))
        .collect();
    let amounts: Vec<Cents> = match candidate.value {
        Amount::Percentage(percentage) => bases
            .iter()
            .map(|base| (*base as f64 * percentage / 100.0).round() as Cents)
            .collect(),
        Amount::Fixed { cents, each: true } => covered
            .iter()
            .map(|(_, quantity)| cents * Cents::from(*quantity))
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

/// What an order candidate takes off each line: the amount is shared between the lines in
/// proportion to what they cost after the line discounts.
fn plan_order(lines: &[ResultLine], candidate: &Candidate) -> Vec<(usize, Cents)> {
    let (excluded_lines, excluded_variants): (Vec<&String>, Vec<u64>) = candidate
        .targets
        .iter()
        .filter_map(|target| match target {
            Target::Order {
                excluded_lines,
                excluded_variants,
            } => Some((excluded_lines, excluded_variants)),
            _ => None,
        })
        .fold(
            (Vec::new(), Vec::new()),
            |(mut lines, mut variants), (l, v)| {
                lines.extend(l);
                variants.extend(v);
                (lines, variants)
            },
        );
    if !candidate
        .targets
        .iter()
        .any(|target| matches!(target, Target::Order { .. }))
    {
        return Vec::new();
    }
    let included: Vec<usize> = lines
        .iter()
        .enumerate()
        .filter(|(_, line)| {
            !excluded_lines.contains(&&line.id) && !excluded_variants.contains(&line.variant_id)
        })
        .map(|(index, _)| index)
        .collect();
    let weights: Vec<Cents> = included.iter().map(|index| lines[*index].total).collect();
    let base: Cents = weights.iter().sum();
    let amount = match candidate.value {
        Amount::Percentage(percentage) => (base as f64 * percentage / 100.0).round() as Cents,
        Amount::Fixed { cents, .. } => cents.min(base),
        Amount::None => 0,
    };
    included
        .into_iter()
        .zip(distribute(amount, &weights))
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

fn cents_of(value: &Value) -> Option<Cents> {
    number_of(value).map(|amount| (amount * 100.0).round() as Cents)
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
        candidate(&raw)
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
        }));
        assert_eq!(candidate.message.as_deref(), Some("Ten off"));
        assert_eq!(plan_product(&lines, &candidate), vec![(0, 400)]);
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
        assert_eq!(cents_of(&json!("19.99")), Some(1999));
        assert_eq!(cents_of(&json!(19.995)), Some(2000));
        assert_eq!(cents_of(&json!(5)), Some(500));
        assert_eq!(cents_of(&json!(null)), None);
    }
}
