// What the app does with the Admin API, family by family. Each function takes the `admin`
// context of `authenticate.admin` (or of a webhook) and answers what it did, for the front end
// (here, the end-to-end runner) to check.

export class AdminError extends Error {
  constructor(message, details) {
    super(message);
    this.details = details;
  }
}

// `admin.graphql` answers a Response (the React Router API); the data, or the user errors of
// the mutation, as an exception.
export async function gql(admin, query, variables, { mutation } = {}) {
  const response = await admin.graphql(query, { variables });
  const body = await response.json();
  if (body.errors) throw new AdminError("GraphQL errors", body.errors);
  if (mutation) {
    const payload = body.data[mutation];
    if (payload?.userErrors?.length) {
      throw new AdminError(`${mutation}: ${payload.userErrors.map((e) => e.message).join("; ")}`, payload.userErrors);
    }
    return { ...payload, extensions: body.extensions };
  }
  return { ...body.data, extensions: body.extensions };
}

// Every product, five at a time, following `pageInfo`.
export async function listProducts(admin) {
  const titles = [];
  let after = null;
  let pages = 0;
  for (;;) {
    const { products } = await gql(
      admin,
      `#graphql
      query Products($after: String) {
        products(first: 5, after: $after, sortKey: TITLE) {
          pageInfo { hasNextPage endCursor }
          nodes { id title handle }
        }
      }`,
      { after },
    );
    pages += 1;
    titles.push(...products.nodes.map((product) => product.title));
    if (!products.pageInfo.hasNextPage) break;
    after = products.pageInfo.endCursor;
  }
  const { productsCount } = await gql(admin, `#graphql
    query { productsCount(limit: null) { count precision } }`);
  return { pages, count: titles.length, productsCount: productsCount.count, titles };
}

// A product as a catalog app makes one: created, priced, described, stocked, published.
export async function createProduct(admin, { title, tag, price, quantity }) {
  const created = await gql(
    admin,
    `#graphql
    mutation Create($product: ProductCreateInput!) {
      productCreate(product: $product) {
        product {
          id handle status
          variants(first: 1) {
            nodes { id inventoryItem { id inventoryLevels(first: 1) { nodes { location { id name } } } } }
          }
        }
        userErrors { field message }
      }
    }`,
    { product: { title, tags: [tag], vendor: "E2E", productType: "Test", status: "ACTIVE" } },
    { mutation: "productCreate" },
  );
  const product = created.product;
  const variant = product.variants.nodes[0];

  await gql(
    admin,
    `#graphql
    mutation Price($productId: ID!, $variants: [ProductVariantsBulkInput!]!) {
      productVariantsBulkUpdate(productId: $productId, variants: $variants) {
        productVariants { id price }
        userErrors { field message }
      }
    }`,
    {
      productId: product.id,
      variants: [{ id: variant.id, price, inventoryItem: { sku: `E2E-${Date.now()}`, tracked: true } }],
    },
    { mutation: "productVariantsBulkUpdate" },
  );

  const updated = await gql(
    admin,
    `#graphql
    mutation Update($product: ProductUpdateInput!) {
      productUpdate(product: $product) {
        product { id title descriptionHtml }
        userErrors { field message }
      }
    }`,
    { product: { id: product.id, title: `${title} (updated)`, descriptionHtml: "<p>Made by the end-to-end test.</p>" } },
    { mutation: "productUpdate" },
  );

  // The shop's primary location (`location` without an id), where a new variant is stocked.
  const { location } = await gql(admin, `#graphql
    query { location { id name } }`);
  const stockedAt = variant.inventoryItem.inventoryLevels.nodes.map((level) => level.location.id);
  if (!stockedAt.includes(location.id)) {
    throw new AdminError(`the new variant is stocked at ${stockedAt}, not at the primary location ${location.id}`);
  }
  const inventory = await gql(
    admin,
    `#graphql
    mutation Stock($input: InventorySetQuantitiesInput!, $key: String!) {
      inventorySetQuantities(input: $input) @idempotent(key: $key) {
        inventoryAdjustmentGroup { reason changes { name delta quantityAfterChange } }
        userErrors { field message code }
      }
    }`,
    {
      key: `e2e-stock-${product.id}`,
      input: {
        name: "available",
        reason: "correction",
        quantities: [{ inventoryItemId: variant.inventoryItem.id, locationId: location.id, quantity, changeFromQuantity: 0 }],
      },
    },
    { mutation: "inventorySetQuantities" },
  );

  const { publications } = await gql(admin, `#graphql
    query { publications(first: 10) { nodes { id name } } }`);
  const onlineStore = publications.nodes.find((publication) => publication.name === "Online Store");
  await gql(
    admin,
    `#graphql
    mutation Publish($id: ID!, $input: [PublicationInput!]!) {
      publishablePublish(id: $id, input: $input) {
        publishable { ... on Product { publishedAt onlineStoreUrl } }
        userErrors { field message }
      }
    }`,
    { id: product.id, input: [{ publicationId: onlineStore.id }] },
    { mutation: "publishablePublish" },
  );

  const { product: read } = await gql(
    admin,
    `#graphql
    query Read($id: ID!) {
      product(id: $id) {
        id title handle status totalInventory onlineStoreUrl
        variants(first: 1) { nodes { id price inventoryQuantity } }
      }
    }`,
    { id: product.id },
  );
  return {
    product: read,
    updatedTitle: updated.product.title,
    inventoryChanges: inventory.inventoryAdjustmentGroup.changes,
    location,
  };
}

export async function deleteProduct(admin, { id }) {
  return gql(
    admin,
    `#graphql
    mutation Delete($input: ProductDeleteInput!) {
      productDelete(input: $input) { deletedProductId userErrors { field message } }
    }`,
    { input: { id } },
    { mutation: "productDelete" },
  );
}

// A customer with a metafield, and an entry of the app's metaobject type.
export async function createCustomer(admin, { email }) {
  const created = await gql(
    admin,
    `#graphql
    mutation Customer($input: CustomerInput!) {
      customerCreate(input: $input) {
        customer { id email firstName state }
        userErrors { field message }
      }
    }`,
    { input: { email, firstName: "Ada", lastName: "Lovelace", tags: ["e2e"] } },
    { mutation: "customerCreate" },
  );
  const customer = created.customer;
  const set = await gql(
    admin,
    `#graphql
    mutation Metafields($metafields: [MetafieldsSetInput!]!) {
      metafieldsSet(metafields: $metafields) {
        metafields { namespace key value type }
        userErrors { field message code }
      }
    }`,
    {
      metafields: [
        { ownerId: customer.id, namespace: "loyalty", key: "tier", type: "single_line_text_field", value: "gold" },
      ],
    },
    { mutation: "metafieldsSet" },
  );

  const { metaobjectDefinitionByType } = await gql(admin, `#graphql
    query { metaobjectDefinitionByType(type: "$app:review") { id type } }`);
  if (!metaobjectDefinitionByType) {
    await gql(
      admin,
      `#graphql
      mutation Definition($definition: MetaobjectDefinitionCreateInput!) {
        metaobjectDefinitionCreate(definition: $definition) {
          metaobjectDefinition { id type }
          userErrors { field message code }
        }
      }`,
      {
        definition: {
          type: "$app:review",
          name: "Review",
          displayNameKey: "title",
          fieldDefinitions: [
            { key: "title", name: "Title", type: "single_line_text_field", required: true },
            { key: "rating", name: "Rating", type: "number_integer" },
          ],
        },
      },
      { mutation: "metaobjectDefinitionCreate" },
    );
  }
  const upserted = await gql(
    admin,
    `#graphql
    mutation Review($handle: MetaobjectHandleInput!, $metaobject: MetaobjectUpsertInput!) {
      metaobjectUpsert(handle: $handle, metaobject: $metaobject) {
        metaobject { id handle type displayName fields { key value } }
        userErrors { field message code }
      }
    }`,
    {
      handle: { type: "$app:review", handle: "first-review" },
      metaobject: { fields: [{ key: "title", value: "Works offline" }, { key: "rating", value: "5" }] },
    },
    { mutation: "metaobjectUpsert" },
  );
  const { customer: read } = await gql(
    admin,
    `#graphql
    query Read($id: ID!) {
      customer(id: $id) { id email tags metafield(namespace: "loyalty", key: "tier") { value } }
    }`,
    { id: customer.id },
  );
  return { customer: read, metafields: set.metafields, metaobject: upserted.metaobject };
}

// An order the app imports (`orderCreate`), or one placed at the checkout: fulfilled, then one
// unit refunded.
export async function processOrder(admin, { orderId, variantId, email }) {
  if (!orderId) {
    const created = await gql(
      admin,
      `#graphql
      mutation Order($order: OrderCreateOrderInput!, $options: OrderCreateOptionsInput) {
        orderCreate(order: $order, options: $options) {
          order { id name displayFinancialStatus }
          userErrors { field message }
        }
      }`,
      {
        order: { email, lineItems: [{ variantId, quantity: 2 }], financialStatus: "PAID" },
        options: { inventoryBehaviour: "DECREMENT_OBEYING_POLICY", sendReceipt: false },
      },
      { mutation: "orderCreate" },
    );
    orderId = created.order.id;
  }
  const { order } = await gql(
    admin,
    `#graphql
    query Order($id: ID!) {
      order(id: $id) {
        id name displayFinancialStatus displayFulfillmentStatus
        lineItems(first: 10) { nodes { id quantity title } }
        fulfillmentOrders(first: 5) { nodes { id status } }
      }
    }`,
    { id: orderId },
  );
  const fulfilled = await gql(
    admin,
    `#graphql
    mutation Fulfill($fulfillment: FulfillmentInput!) {
      fulfillmentCreate(fulfillment: $fulfillment) {
        fulfillment { id status trackingInfo { number company } }
        userErrors { field message }
      }
    }`,
    {
      fulfillment: {
        lineItemsByFulfillmentOrder: order.fulfillmentOrders.nodes
          .filter((fo) => fo.status === "OPEN")
          .map((fo) => ({ fulfillmentOrderId: fo.id })),
        trackingInfo: { number: "1Z999", company: "UPS" },
        notifyCustomer: false,
      },
    },
    { mutation: "fulfillmentCreate" },
  );
  const line = order.lineItems.nodes[0];
  const refundLineItems = [{ lineItemId: line.id, quantity: 1, restockType: "NO_RESTOCK" }];
  const { order: suggested } = await gql(
    admin,
    `#graphql
    query Suggested($id: ID!, $lines: [RefundLineItemInput!]) {
      order(id: $id) {
        suggestedRefund(refundLineItems: $lines) {
          amountSet { shopMoney { amount currencyCode } }
          suggestedTransactions { gateway kind amountSet { shopMoney { amount } } parentTransaction { id } }
        }
      }
    }`,
    { id: orderId, lines: refundLineItems },
  );
  const refunded = await gql(
    admin,
    `#graphql
    mutation Refund($input: RefundInput!) {
      refundCreate(input: $input) {
        refund { id totalRefundedSet { shopMoney { amount currencyCode } } }
        order { id displayFinancialStatus }
        userErrors { field message }
      }
    }`,
    {
      input: {
        orderId,
        refundLineItems,
        transactions: suggested.suggestedRefund.suggestedTransactions.map((t) => ({
          orderId,
          gateway: t.gateway,
          kind: "REFUND",
          amount: t.amountSet.shopMoney.amount,
          parentId: t.parentTransaction?.id,
        })),
      },
    },
    { mutation: "refundCreate" },
  );
  return {
    order: { id: order.id, name: order.name, financialStatus: order.displayFinancialStatus },
    fulfillment: fulfilled.fulfillment,
    refund: refunded.refund,
    financialStatusAfterRefund: refunded.order.displayFinancialStatus,
  };
}

// An automatic discount run by the app's discount Function, configured by its metafield.
export async function createDiscount(admin, { tag, percentage }) {
  const { shopifyFunctions } = await gql(admin, `#graphql
    query { shopifyFunctions(first: 25) { nodes { id handle apiType title } } }`);
  const fn = shopifyFunctions.nodes.find((node) => node.handle === "discount-tag-percentage");
  const created = await gql(
    admin,
    `#graphql
    mutation Discount($discount: DiscountAutomaticAppInput!) {
      discountAutomaticAppCreate(automaticAppDiscount: $discount) {
        automaticAppDiscount { discountId title status discountClasses appDiscountType { functionId } }
        userErrors { field message code }
      }
    }`,
    {
      discount: {
        title: `${percentage}% off ${tag}`,
        functionHandle: fn.handle,
        discountClasses: ["PRODUCT"],
        startsAt: new Date(Date.now() - 60_000).toISOString(),
        combinesWith: { orderDiscounts: false, productDiscounts: false, shippingDiscounts: true },
        metafields: [
          {
            namespace: "$app:config",
            key: "settings",
            type: "json",
            value: JSON.stringify({ percentage, tags: [tag], collections: [] }),
          },
        ],
      },
    },
    { mutation: "discountAutomaticAppCreate" },
  );
  return { function: fn, discount: created.automaticAppDiscount };
}

// A bulk export of the catalog, as JSON Lines.
export async function bulkExport(admin) {
  const started = await gql(
    admin,
    `#graphql
    mutation {
      bulkOperationRunQuery(query: """
        { products { edges { node { id title variants { edges { node { id sku price } } } } } } }
      """) {
        bulkOperation { id status }
        userErrors { field message code }
      }
    }`,
    {},
    { mutation: "bulkOperationRunQuery" },
  );
  const id = started.bulkOperation.id;
  let operation;
  for (let attempt = 0; attempt < 50; attempt += 1) {
    ({ node: operation } = await gql(
      admin,
      `#graphql
      query Poll($id: ID!) {
        node(id: $id) { ... on BulkOperation { id status errorCode objectCount fileSize url partialDataUrl } }
      }`,
      { id },
    ));
    if (!["CREATED", "RUNNING"].includes(operation.status)) break;
    await new Promise((resolve) => setTimeout(resolve, 200));
  }
  if (operation.status !== "COMPLETED") throw new AdminError(`bulk operation ${operation.status}`, operation);
  const text = await (await fetch(operation.url)).text();
  const lines = text.trim().split("\n").map((line) => JSON.parse(line));
  return {
    initialStatus: started.bulkOperation.status,
    operation,
    lines: lines.length,
    products: lines.filter((line) => !line.__parentId).length,
    variants: lines.filter((line) => line.__parentId).length,
  };
}

// A bulk import: the variables of each productCreate as JSON Lines, uploaded to a staged target,
// then bulkOperationRunMutation, as shopify.dev's "Bulk import data" does.
export async function bulkImport(admin, { titles }) {
  const jsonl = titles.map((title) => JSON.stringify({ product: { title, tags: ["e2e-bulk"] } })).join("\n");
  const staged = await gql(
    admin,
    `#graphql
    mutation Stage($input: [StagedUploadInput!]!) {
      stagedUploadsCreate(input: $input) {
        stagedTargets { url resourceUrl parameters { name value } }
        userErrors { field message }
      }
    }`,
    { input: [{ filename: "products.jsonl", mimeType: "text/jsonl", resource: "BULK_MUTATION_VARIABLES", httpMethod: "POST" }] },
    { mutation: "stagedUploadsCreate" },
  );
  const target = staged.stagedTargets[0];
  const form = new FormData();
  for (const { name, value } of target.parameters) form.append(name, value);
  form.append("file", new Blob([jsonl], { type: "text/jsonl" }), "products.jsonl");
  const upload = await fetch(target.url, { method: "POST", body: form });
  if (!upload.ok) throw new AdminError(`upload answered ${upload.status}`, await upload.text());
  const key = target.parameters.find((parameter) => parameter.name === "key").value;
  const started = await gql(
    admin,
    `#graphql
    mutation Import($mutation: String!, $path: String!) {
      bulkOperationRunMutation(mutation: $mutation, stagedUploadPath: $path) {
        bulkOperation { id status }
        userErrors { field message code }
      }
    }`,
    {
      mutation: `mutation call($product: ProductCreateInput!) { productCreate(product: $product) { product { id title } userErrors { message field } } }`,
      path: key,
    },
    { mutation: "bulkOperationRunMutation" },
  );
  let operation;
  for (let attempt = 0; attempt < 50; attempt += 1) {
    ({ node: operation } = await gql(
      admin,
      `#graphql
      query Poll($id: ID!) { node(id: $id) { ... on BulkOperation { id status type errorCode objectCount url } } }`,
      { id: started.bulkOperation.id },
    ));
    if (!["CREATED", "RUNNING"].includes(operation.status)) break;
    await new Promise((resolve) => setTimeout(resolve, 200));
  }
  if (operation.status !== "COMPLETED") throw new AdminError(`bulk operation ${operation.status}`, operation);
  const lines = (await (await fetch(operation.url)).text()).trim().split("\n").map((line) => JSON.parse(line));
  return { operation, results: lines };
}

// A file uploaded the way Shopify documents it: a staged target, a multipart POST of the bytes,
// then fileCreate with the target's resourceUrl.
export async function uploadFile(admin, { filename, bytes, mimeType }) {
  const staged = await gql(
    admin,
    `#graphql
    mutation Stage($input: [StagedUploadInput!]!) {
      stagedUploadsCreate(input: $input) {
        stagedTargets { url resourceUrl parameters { name value } }
        userErrors { field message }
      }
    }`,
    { input: [{ filename, mimeType, resource: "IMAGE", httpMethod: "POST", fileSize: String(bytes.length) }] },
    { mutation: "stagedUploadsCreate" },
  );
  const target = staged.stagedTargets[0];
  const form = new FormData();
  for (const { name, value } of target.parameters) form.append(name, value);
  form.append("file", new Blob([bytes], { type: mimeType }), filename);
  const upload = await fetch(target.url, { method: "POST", body: form });
  if (!upload.ok) throw new AdminError(`upload answered ${upload.status}`, await upload.text());

  const created = await gql(
    admin,
    `#graphql
    mutation File($files: [FileCreateInput!]!) {
      fileCreate(files: $files) {
        files { id fileStatus alt }
        userErrors { field message code }
      }
    }`,
    { files: [{ originalSource: target.resourceUrl, contentType: "IMAGE", alt: "Uploaded by the end-to-end test" }] },
    { mutation: "fileCreate" },
  );
  const id = created.files[0].id;
  let file;
  for (let attempt = 0; attempt < 25; attempt += 1) {
    ({ node: file } = await gql(
      admin,
      `#graphql
      query File($id: ID!) {
        node(id: $id) { ... on MediaImage { id fileStatus alt image { url width height } } }
      }`,
      { id },
    ));
    if (file.fileStatus === "READY" || file.fileStatus === "FAILED") break;
    await new Promise((resolve) => setTimeout(resolve, 200));
  }
  let downloaded = null;
  if (file.image?.url) {
    const answer = await fetch(file.image.url);
    downloaded = answer.ok ? Buffer.from(await answer.arrayBuffer()) : null;
  }
  return {
    target: { url: target.url, resourceUrl: target.resourceUrl },
    uploadStatus: upload.status,
    file,
    sameBytes: downloaded ? Buffer.compare(downloaded, Buffer.from(bytes)) === 0 : false,
  };
}

// A subscription the app makes at run time, besides those of shopify.app.toml.
export async function subscribe(admin, { topic, uri }) {
  const created = await gql(
    admin,
    `#graphql
    mutation Subscribe($topic: WebhookSubscriptionTopic!, $subscription: WebhookSubscriptionInput!) {
      webhookSubscriptionCreate(topic: $topic, webhookSubscription: $subscription) {
        webhookSubscription { id topic uri format }
        userErrors { field message }
      }
    }`,
    { topic, subscription: { uri, format: "JSON" } },
    { mutation: "webhookSubscriptionCreate" },
  );
  const { webhookSubscriptions } = await gql(admin, `#graphql
    query { webhookSubscriptions(first: 25) { nodes { id topic uri } } }`);
  return { subscription: created.webhookSubscription, listed: webhookSubscriptions.nodes };
}

export async function updateCustomer(admin, { id, note }) {
  return gql(
    admin,
    `#graphql
    mutation Update($input: CustomerInput!) {
      customerUpdate(input: $input) { customer { id note } userErrors { field message } }
    }`,
    { input: { id, note } },
    { mutation: "customerUpdate" },
  );
}

// The cost Shopify reports, read from the answer's extensions.
export async function queryCost(admin) {
  const answer = await gql(admin, `#graphql
    query { shop { name myshopifyDomain } products(first: 10) { nodes { id variants(first: 5) { nodes { id } } } } }`);
  return { shop: answer.shop, cost: answer.extensions?.cost };
}
