//! Cart filters.

use slt_liquid::{Context, Environment, FilterArgs, Result, Value};

use crate::drops::cart::{CartDrop, LineItemDrop};
use crate::drops::product::{ProductDrop, VariantDrop};

fn line_items(cart: &Value) -> Vec<Value> {
    cart.get("items")
        .items()
        .map(|items| items.as_ref().clone())
        .unwrap_or_default()
}

fn item_count_for_variant(input: &Value, args: &FilterArgs, _ctx: &Context) -> Result<Value> {
    if input.downcast::<CartDrop>().is_none() {
        return Ok(Value::Int(0));
    }
    let wanted = slt_liquid::number::to_number(&args.at(0)).to_f64() as u64;
    let count: i64 = line_items(input)
        .iter()
        .filter(|item| {
            item.downcast::<LineItemDrop>()
                .and_then(LineItemDrop::location)
                .is_some_and(|(_, _, id)| id == wanted)
        })
        .map(|item| item.get("quantity").as_i64().unwrap_or(0))
        .sum();
    Ok(Value::Int(count))
}

fn line_items_for(input: &Value, args: &FilterArgs, _ctx: &Context) -> Result<Value> {
    if input.downcast::<CartDrop>().is_none() {
        return Ok(Value::array(Vec::new()));
    }
    let target = args.at(0);
    let matches = |product: usize, _variant: usize, id: u64| {
        if let Some(wanted) = target.downcast::<ProductDrop>() {
            wanted.index == product
        } else if let Some(wanted) = target.downcast::<VariantDrop>() {
            wanted.variant().id == id
        } else {
            false
        }
    };
    Ok(Value::array(
        line_items(input)
            .into_iter()
            .filter(|item| {
                item.downcast::<LineItemDrop>()
                    .and_then(LineItemDrop::location)
                    .is_some_and(|(product, variant, id)| matches(product, variant, id))
            })
            .collect(),
    ))
}

pub(super) fn register(env: &mut Environment) {
    env.register_filter("item_count_for_variant", item_count_for_variant);
    env.register_filter("line_items_for", line_items_for);
}
