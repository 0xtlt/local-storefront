// @ts-check

// Refuses an order of more items than the JSON metafield `$app:config.settings` of the
// validation allows:
//
//   {"maximumUnits": 30}
//
// What it shows: a Function written in plain JavaScript (types from JSDoc), built by Shopify's
// CLI (esbuild, then Javy) or by lsf, runs as a Rust one does.

/**
 * @typedef {import("../generated/api").CartValidationsGenerateRunInput} CartValidationsGenerateRunInput
 * @typedef {import("../generated/api").CartValidationsGenerateRunResult} CartValidationsGenerateRunResult
 */

/**
 * @param {CartValidationsGenerateRunInput} input
 * @returns {CartValidationsGenerateRunResult}
 */
export function cartValidationsGenerateRun(input) {
  const settings = input.validation.metafield?.jsonValue ?? {};
  const maximum = settings.maximumUnits;
  if (typeof maximum !== "number") {
    return { operations: [] };
  }
  const units = input.cart.lines.reduce((total, line) => total + line.quantity, 0);
  const products = input.cart.lines.flatMap((line) =>
    line.merchandise.__typename === "ProductVariant" ? [line.merchandise.product.title] : [],
  );
  console.log(`${units} items: ${products.join(", ")}`);
  if (units <= maximum) {
    return { operations: [] };
  }
  return {
    operations: [
      {
        validationAdd: {
          errors: [
            {
              message: `At most ${maximum} items per order (${units} in the cart)`,
              target: "$.cart",
            },
          ],
        },
      },
    ],
  };
}
