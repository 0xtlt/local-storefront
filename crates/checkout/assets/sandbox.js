// Runs inside the sandboxed iframe of one extension at one target. The page sends `init`
// (which bundle, which target, the state); we define the `shopify` global, load the bundle
// as a classic script (what `shopify app build` writes is an IIFE), then run the function the
// bundle registered with `shopify.extend` for the target.
//
// Messages to the page: `ready`, `resize`, `overlay`, `change`, `storage`, `sessionToken`,
// `analytics`, `log`, `error`, `interceptors`, `intercepted`, `dismiss`, `reload`, `navigate`.
// With `probe` in `init` (the home page), the bundle is only loaded, and the frame answers
// `probed`: whether the bundle loaded, and the targets it registered.
(function (global) {
  'use strict';
  const Sandbox = global.LsfSandbox;
  const post = (message) => global.parent.postMessage({ lsf: true, ...message }, '*');

  let runtime = null;

  function fail(message, error) {
    post({ type: 'error', message: String(message), stack: error && error.stack });
  }

  function loadScript(url) {
    return new Promise((resolve, reject) => {
      const script = document.createElement('script');
      script.src = url;
      script.onload = resolve;
      script.onerror = () =>
        reject(new Error('Could not load ' + url + '. Run `shopify app build` so the extension has a bundle in dist/.'));
      document.head.appendChild(script);
    });
  }

  /** The frame is as tall as what the extension drew. */
  function reportSize() {
    let last = -1;
    const report = () => {
      const height = Math.ceil(document.body.getBoundingClientRect().height);
      if (height !== last) {
        last = height;
        post({ type: 'resize', height });
      }
    };
    if (global.ResizeObserver) new global.ResizeObserver(report).observe(document.body);
    report();
  }

  async function run(init) {
    const render = runtime.registered.get(init.target);
    if (!render) {
      const known = [...runtime.registered.keys()];
      fail(
        'The bundle did not register ' + init.target + ' with shopify.extend (it registered: ' +
          (known.length ? known.join(', ') : 'nothing') + ').',
      );
      return;
    }
    await render();
  }

  async function start(init) {
    document.documentElement.setAttribute('data-variant', init.variant || 'standard');
    runtime = Sandbox.createRuntime({ post, init });
    global.shopify = runtime.shopify;
    Sandbox.onOverlay = (open) => {
      // The frame covers the window while an overlay is open: show the overlay only.
      document.documentElement.classList.toggle('lsf-overlay', open);
      post({ type: 'overlay', open });
    };
    Sandbox.navigate = (url, target) => post({ type: 'navigate', url, target });
    reportSize();
    // The home page only asks what the bundle registers: it loads, and renders nothing.
    const probed = (loaded) => post({ type: 'probed', loaded, targets: [...runtime.registered.keys()] });
    try {
      await loadScript(init.script);
    } catch (error) {
      fail(error && error.message ? error.message : error, error);
      if (init.probe) probed(false);
      return;
    }
    if (init.probe) {
      probed(true);
      return;
    }
    try {
      await run(init);
    } catch (error) {
      fail(error && error.message ? error.message : error, error);
    }
  }

  global.addEventListener('message', (event) => {
    const data = event.data;
    if (event.source !== global.parent || !data || data.lsf !== true) return;
    if (data.type === 'init') {
      if (!runtime) start(data);
    } else if (runtime) {
      runtime.receive(data);
    }
  });
  global.addEventListener('error', (event) => fail(event.message, event.error));
  global.addEventListener('unhandledrejection', (event) => {
    const reason = event.reason;
    fail('Unhandled rejection: ' + (reason && reason.message ? reason.message : reason), reason);
  });

  post({ type: 'ready' });
})(globalThis);
