// The app's configuration, read from where lsf reads it: the client id and the scopes from
// shopify.app.toml, the client secret from SHOPIFY_API_SECRET (the environment, else the app's
// .env). `shopify app dev` gives a backend the same values as SHOPIFY_API_KEY, SCOPES and
// SHOPIFY_APP_URL; those win when they are set.
import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { parse } from "smol-toml";

export const appRoot = join(dirname(fileURLToPath(import.meta.url)), "..");

function dotenv(file) {
  const values = {};
  let text = "";
  try {
    text = readFileSync(file, "utf8");
  } catch {
    return values;
  }
  for (const line of text.split("\n")) {
    const match = line.match(/^\s*(?:export\s+)?([A-Z0-9_]+)\s*=\s*(.*?)\s*$/);
    if (match) values[match[1]] = match[2].replace(/^(["'])(.*)\1$/, "$2");
  }
  return values;
}

const toml = parse(readFileSync(join(appRoot, "shopify.app.toml"), "utf8"));
const env = { ...dotenv(join(appRoot, ".env")), ...process.env };
const port = Number(env.PORT ?? 3000);

export const config = {
  apiKey: env.SHOPIFY_API_KEY ?? toml.client_id,
  apiSecretKey: env.SHOPIFY_API_SECRET ?? "",
  scopes: (env.SCOPES ?? toml.access_scopes?.scopes ?? "").split(",").filter(Boolean),
  appUrl: env.SHOPIFY_APP_URL ?? `http://localhost:${port}`,
  port,
  // Where lsf listens, when the backend runs against it (unset: Shopify).
  lsfUrl: env.LSF_URL,
  toml,
};

if (!config.apiSecretKey) {
  throw new Error("SHOPIFY_API_SECRET is not set (in the environment or in the app's .env)");
}
