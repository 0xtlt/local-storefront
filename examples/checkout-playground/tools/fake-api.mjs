#!/usr/bin/env node
// The server the network Functions of the playground call (`discount-network` and
// `validation-network`, through their `.fetch` targets). No dependency:
//
//   node tools/fake-api.mjs          # http://localhost:8787
//   PORT=9000 node tools/fake-api.mjs
//
// Without it, the checkout still runs: their requests answer 502, as on Shopify when the
// connection fails, and the Functions say so in their logs.
//
// Checkout UI extensions call it too, from their sandboxed frames: those have an opaque origin,
// so every answer allows any origin and preflights (OPTIONS) are answered.
import { createServer } from "node:http";

const port = Number(process.env.PORT ?? 8787);
// The SKUs the stock service says are out of stock.
const OUT_OF_STOCK = new Set(["MUG-SLT", "CNDL-FIG"]);

const CORS = { "access-control-allow-origin": "*" };

function send(response, status, body) {
  response.writeHead(status, { ...CORS, "content-type": "application/json; charset=utf-8" });
  response.end(JSON.stringify(body));
}

/** The JSON object of the body, or null when the body is not one. */
async function readJson(request) {
  let text = "";
  for await (const chunk of request) text += chunk;
  try {
    const value = JSON.parse(text || "{}");
    return value !== null && typeof value === "object" && !Array.isArray(value) ? value : null;
  } catch {
    return null;
  }
}

async function handle(request, response) {
  const url = new URL(request.url, `http://${request.headers.host ?? "localhost"}`);
  console.log(`${request.method} ${url.pathname}${url.search}`);

  if (request.method === "OPTIONS") {
    response.writeHead(204, {
      ...CORS,
      "access-control-allow-headers": "*",
      "access-control-allow-methods": "GET, POST, OPTIONS",
    });
    return response.end();
  }

  // discount-network: the percentage of the partner price, for the cart it is told about.
  if (request.method === "POST" && url.pathname === "/discount") {
    const cart = await readJson(request);
    if (!cart) return send(response, 400, { error: "the body is not a JSON object" });
    const percentage = cart.subtotal >= 100 ? 15 : 12;
    console.log(`  cart of ${cart.items} items, ${cart.subtotal} ${cart.currency}: ${percentage}%`);
    return send(response, 200, { percentage, message: "Partner price" });
  }

  // validation-network: which of the SKUs asked about are out of stock.
  if (request.method === "GET" && url.pathname === "/stock") {
    const skus = (url.searchParams.get("skus") ?? "").split(",").filter(Boolean);
    const unavailable = skus.filter((sku) => OUT_OF_STOCK.has(sku));
    console.log(`  out of stock: ${unavailable.join(", ") || "none"}`);
    return send(response, 200, { unavailable });
  }

  send(response, 404, { error: `nothing at ${request.method} ${url.pathname}` });
}

// On the loopback addresses only, both of them: `localhost` is one or the other, depending on
// the machine. A machine without IPv6 has the first one only.
let listening = 0;
for (const host of ["127.0.0.1", "::1"]) {
  const server = createServer((request, response) => {
    handle(request, response).catch((error) => {
      console.error(error);
      if (!response.headersSent) send(response, 500, { error: String(error?.message ?? error) });
      else response.destroy();
    });
  });
  server.on("error", (error) => {
    if (host === "::1" && (error.code === "EADDRNOTAVAIL" || error.code === "EAFNOSUPPORT")) return;
    throw error;
  });
  server.listen(port, host, () => {
    if (++listening > 1) return;
    console.log(`fake API of the checkout playground: http://localhost:${port}`);
    console.log("  POST /discount  ->  {percentage, message}");
    console.log("  GET  /stock?skus=A,B  ->  {unavailable: [...]}");
  });
}
