// node_modules/@shopify/shopify_function/run.ts
function run_default(userfunction) {
  try {
    ShopifyFunction;
  } catch (e) {
    throw new Error(
      "ShopifyFunction is not defined. Please rebuild your function using the latest version of Shopify CLI."
    );
  }
  const input_obj = ShopifyFunction.readInput();
  const output_obj = userfunction(input_obj);
  ShopifyFunction.writeOutput(output_obj);
}

// src/cart_lines_discounts_generate_run.ts
function cartLinesDiscountsGenerateRun(input) {
  const settings = input.discount.metafield?.jsonValue ?? {};
  const percentage = settings.percentage ?? 0;
  const minimumQuantity = settings.minimumQuantity ?? 2;
  if (!input.discount.discountClasses.includes("PRODUCT" /* Product */) || percentage <= 0) {
    return { operations: [] };
  }
  const targets = [];
  for (const line of input.cart.lines) {
    if (line.quantity < minimumQuantity) {
      continue;
    }
    if (line.merchandise.__typename === "ProductVariant") {
      console.log(`${line.merchandise.product.title}: ${line.quantity} units`);
    }
    targets.push({ cartLine: { id: line.id } });
  }
  if (targets.length === 0) {
    return { operations: [] };
  }
  return {
    operations: [
      {
        productDiscountsAdd: {
          selectionStrategy: "FIRST" /* First */,
          candidates: [
            {
              targets,
              message: `${percentage}% off ${minimumQuantity} or more`,
              value: { percentage: { value: percentage } }
            }
          ]
        }
      }
    ]
  };
}

// <stdin>
function cartLinesDiscountsGenerateRun2() {
  return run_default(cartLinesDiscountsGenerateRun);
}
export {
  cartLinesDiscountsGenerateRun2 as cartLinesDiscountsGenerateRun
};
