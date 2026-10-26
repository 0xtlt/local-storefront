// A static preview of the checkout page: serves the page, its assets and the built bundles of
// the fixture extensions at the paths the real server uses, with `mock-engine.mjs` standing in
// for the checkout.
//
//   node crates/checkout/assets/preview/serve.mjs [--port 9293] [--variant standard|shop]
//        [--extensions DIR]   # an app's extensions/ directory, default: the test fixtures
//        [--state FILE]       # the checkout to start from, default: sample-state.json
//
// Then open http://127.0.0.1:9293/ (add `?variant=shop` for the Shop skin).

import { readFile } from 'node:fs/promises';
import { createServer } from 'node:http';
import { dirname, extname, join, normalize, sep } from 'node:path';
import { fileURLToPath } from 'node:url';
import { complete, createCheckout, update } from './mock-engine.mjs';

const here = dirname(fileURLToPath(import.meta.url));
const assets = join(here, '..');
const MIME = {
  '.html': 'text/html; charset=utf-8',
  '.js': 'text/javascript; charset=utf-8',
  '.mjs': 'text/javascript; charset=utf-8',
  '.css': 'text/css; charset=utf-8',
  '.json': 'application/json; charset=utf-8',
};

/** The state JSON as it goes into `<script type="application/json">`. */
export function embed(state) {
  const separators = { [String.fromCharCode(0x2028)]: '\\u2028', [String.fromCharCode(0x2029)]: '\\u2029' };
  return JSON.stringify(state)
    .replace(/</g, '\\u003c')
    .replace(/>/g, '\\u003e')
    .replace(/&/g, '\\u0026')
    .replace(new RegExp(`[${Object.keys(separators).join('')}]`, 'g'), (character) => separators[character]);
}

const publicState = ({ selection, ...state }) => state;

function notFound(response) {
  response.writeHead(404, { 'content-type': 'text/plain' });
  response.end('Not found');
}

/** Serves `file` from `directory` only if it stays inside it. */
async function sendFile(response, directory, file) {
  const path = normalize(join(directory, file));
  if (!path.startsWith(directory + sep)) return notFound(response);
  try {
    const body = await readFile(path);
    response.writeHead(200, { 'content-type': MIME[extname(path)] ?? 'application/octet-stream', 'cache-control': 'no-store' });
    return response.end(body);
  } catch {
    return notFound(response);
  }
}

const json = (response, status, body) => {
  response.writeHead(status, { 'content-type': MIME['.json'], 'cache-control': 'no-store' });
  response.end(JSON.stringify(body));
};

async function readBody(request) {
  let text = '';
  for await (const chunk of request) text += chunk;
  return text ? JSON.parse(text) : {};
}

export async function startPreview({
  port = 9293,
  variant = 'standard',
  extensions = join(assets, '..', 'tests', 'fixtures', 'app', 'extensions'),
  sampleFile = join(here, 'sample-state.json'),
} = {}) {
  const sample = JSON.parse(await readFile(sampleFile, 'utf8'));
  let current = null;
  const origin = (request) => `http://${request.headers.host}`;

  const server = createServer(async (request, response) => {
    const url = new URL(request.url, origin(request));
    const path = decodeURIComponent(url.pathname);
    try {
      if (path === '/' || path === '/checkout') {
        response.writeHead(302, { location: `/checkouts/cn/preview${url.search}` });
        return response.end();
      }
      if (path.startsWith('/__lsf/checkout/assets/')) {
        return await sendFile(response, assets, path.slice('/__lsf/checkout/assets/'.length));
      }
      const bundle = path.match(/^\/__lsf\/extensions\/([\w-]+)\/([\w.-]+)$/);
      if (bundle) return await sendFile(response, join(extensions, bundle[1], 'dist'), bundle[2]);

      const route = path.match(/^\/checkouts\/cn\/([\w-]+)(\.json|\/update|\/complete|\/thank-you)?$/);
      if (!route) return notFound(response);
      const tail = route[2];

      current ??= createCheckout(sample, { origin: origin(request), variant });
      const wanted = url.searchParams.get('variant');
      if (wanted === 'shop' || wanted === 'standard') current = { ...current, variant: wanted };

      if (tail === '.json') return json(response, 200, publicState(current));
      if (tail === '/update') {
        const result = update(current, (await readBody(request)).operations ?? []);
        if (result.error) return json(response, 400, { error: result.error });
        current = result.state;
        return json(response, 200, publicState(current));
      }
      if (tail === '/complete') {
        const result = complete(current);
        if (result.errors) {
          current = result.errors;
          return json(response, 422, publicState(current));
        }
        current = result.state;
        return json(response, 200, { redirect: `${origin(request)}/checkouts/cn/preview/thank-you` });
      }
      if (tail === '/thank-you' && current.status !== 'completed') {
        response.writeHead(302, { location: '/checkouts/cn/preview' });
        return response.end();
      }
      if (url.searchParams.has('reset')) {
        current = createCheckout(sample, { origin: origin(request), variant: current.variant });
      }
      const shell = await readFile(join(assets, 'checkout.html'), 'utf8');
      response.writeHead(200, { 'content-type': MIME['.html'], 'cache-control': 'no-store' });
      return response.end(
        shell.replaceAll('<!--LSF_ASSETS-->', '/__lsf/checkout/assets').replace('<!--LSF_STATE-->', () => embed(publicState(current))),
      );
    } catch (error) {
      return json(response, 500, { error: error.message });
    }
  });

  await new Promise((resolve) => server.listen(port, '127.0.0.1', resolve));
  return { server, port: server.address().port, close: () => new Promise((resolve) => server.close(resolve)) };
}

if (process.argv[1] === fileURLToPath(import.meta.url)) {
  const args = process.argv.slice(2);
  const option = (name) => {
    const at = args.indexOf(name);
    return at >= 0 ? args[at + 1] : undefined;
  };
  const { port } = await startPreview({
    port: Number(option('--port') ?? 9293),
    variant: option('--variant') ?? 'standard',
    ...(option('--extensions') ? { extensions: option('--extensions') } : {}),
    ...(option('--state') ? { sampleFile: option('--state') } : {}),
  });
  console.log(`Checkout preview on http://127.0.0.1:${port}/ (?variant=shop for the Shop skin, ?reset to start over)`);
}
