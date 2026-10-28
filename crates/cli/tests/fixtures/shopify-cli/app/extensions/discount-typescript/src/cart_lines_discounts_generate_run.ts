// A percentage off the lines of a few units or more. The JSON metafield `$app:config.settings`
// of the discount holds the settings:
//
//   {"percentage": 10, "minimumQuantity": 2}
//
// What it shows: a Function written in TypeScript, built by Shopify's CLI (esbuild, then Javy),
// runs in lsf as a Rust one does.

import {
  DiscountClass,
  ProductDiscountSelectionStrategy,
  type CartInput,
  type CartLinesDiscountsGenerateRunResult,
  type ProductDiscountCandidateTarget,
} from "../generated/api";

type Settings = {
  percentage?: number;
  minimumQuantity?: number;
};

export function cartLinesDiscountsGenerateRun(
  input: CartInput,
): CartLinesDiscountsGenerateRunResult {
  const settings: Settings = input.discount.metafield?.jsonValue ?? {};
  const percentage = settings.percentage ?? 0;
  const minimumQuantity = settings.minimumQuantity ?? 2;
  if (
    !input.discount.discountClasses.includes(DiscountClass.Product) ||
    percentage <= 0
  ) {
    return { operations: [] };
  }

  const targets: ProductDiscountCandidateTarget[] = [];
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
          selectionStrategy: ProductDiscountSelectionStrategy.First,
          candidates: [
            {
              targets,
              message: `${percentage}% off ${minimumQuantity} or more`,
              value: { percentage: { value: percentage } },
            },
          ],
        },
      },
    ],
  };
}
