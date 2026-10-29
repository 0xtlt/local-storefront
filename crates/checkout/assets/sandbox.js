// Runs inside the sandboxed iframe of one extension at one target. The page sends `init`
// (which bundle, which target, the state); we define the `shopify` global, load the bundle
// as a classic script (what `shopify app build` writes is an IIFE), then run the function the
// bundle registered with `shopify.extend` for the target.
//
// Messages to the page: `ready`, `resize`, `overlay`, `change`, `storage`, `sessionToken`,
// `analytics`, `log`, `error`, `interceptors`, `intercepted`, `dismiss`, `reload`, `navigate`,
// and `rendered` once the target's function returned (or threw): the page is ready then.
// With `probe` in `init` (the home page), the bundle is only loaded, and the frame answers
// `probed`: whether the bundle loaded, and the targets it registered. With `runnable` (a target
// that renders nothing, the address autocomplete), it waits for `run` and answers `ran` with
// what the extension returned.
(function (global) {
  'use strict';
  const Sandbox = global.LsfSandbox;
  const post = (message) => global.parent.postMessage({ lsf: true, ...message }, '*');

  let runtime = null;
  /** The bundle being loaded: a request to run waits for it. */
  let starting = null;
  /** What the bundle threw while it was evaluated, for the error that says it registered nothing. */
  let loading = null;
  /** `load` while the bundle is evaluated, `run` after. */
  let phase = null;

  /** The text and the stack of what was thrown, whatever it is (an Error, a string, nothing). */
  function describe(error) {
    if (error && typeof error === 'object' && 'message' in error) {
      return { message: String(error.message), stack: typeof error.stack === 'string' ? error.stack : undefined };
    }
    return { message: String(error), stack: undefined };
  }

  function fail(message, error) {
    const stack = error && typeof error.stack === 'string' ? error.stack : undefined;
    post({ type: 'error', message: String(message), stack });
  }

  // The page fetched the bundle and sent its text (`source`): this frame has an opaque origin,
  // and a script it loaded from the server would have its errors hidden ("Script error.") and its
  // unhandled rejections dropped. Run from a `blob:` URL the frame makes, the bundle is a script
  // of the frame's own origin, whose errors the browser shows. `sourceURL` names it in stacks.
  function loadScript(init) {
    return new Promise((resolve, reject) => {
      if (typeof init.source !== 'string') {
        reject(new Error('Could not load ' + init.script + '. Run `shopify app build` so the extension has a bundle in dist/.'));
        return;
      }
      const blob = new Blob([init.source, '\n//# sourceURL=' + init.script], { type: 'text/javascript' });
      const url = URL.createObjectURL(blob);
      const script = document.createElement('script');
      script.src = url;
      script.onload = () => {
        URL.revokeObjectURL(url);
        resolve();
      };
      script.onerror = () => {
        URL.revokeObjectURL(url);
        reject(new Error('Could not run the bundle of ' + init.script + '.'));
      };
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
      const missing =
        'did not register ' + init.target + ' with shopify.extend (it registered: ' +
        (known.length ? known.join(', ') : 'nothing') + ').';
      if (loading) {
        // Why: what it threw while it loaded.
        fail('The bundle threw while it loaded (' + loading.message + '), and ' + missing, loading);
      } else {
        fail('The bundle ' + missing);
      }
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
      loading = null;
      phase = 'load';
      await loadScript(init);
    } catch (error) {
      fail(error && error.message ? error.message : error, error);
      if (init.probe) probed(false);
      else post({ type: 'rendered' });
      return;
    }
    phase = 'run';
    if (init.probe) {
      probed(true);
      return;
    }
    if (init.runnable) {
      post({ type: 'rendered' });
      return;
    }
    try {
      await run(init);
    } catch (error) {
      fail(error && error.message ? error.message : error, error);
    }
    post({ type: 'rendered' });
  }

  global.addEventListener('message', (event) => {
    const data = event.data;
    if (event.source !== global.parent || !data || data.lsf !== true) return;
    if (data.type === 'init') {
      if (!runtime) starting = start(data);
    } else if (data.type === 'run' && runtime) {
      Promise.resolve(starting)
        .then(() => runtime.run(data.payload))
        .then(
          (result) => post({ type: 'ran', id: data.id, result: JSON.parse(JSON.stringify(result === undefined ? null : result)) }),
          (error) => {
            const thrown = describe(error);
            fail(thrown.message, thrown);
            post({ type: 'ran', id: data.id, result: null, error: thrown.message });
          },
        );
    } else if (runtime) {
      runtime.receive(data);
    }
  });
  // What the bundle throws outside of the calls the frame makes (while it is evaluated, in a
  // listener, a timer, a promise nobody waits for): its message and its stack go to the page.
  global.addEventListener('error', (event) => {
    const thrown = event.error !== undefined && event.error !== null ? describe(event.error) : { message: event.message };
    if (phase === 'load' && !loading) loading = { message: thrown.message, stack: thrown.stack };
    fail(thrown.message || event.message, thrown);
  });
  global.addEventListener('unhandledrejection', (event) => {
    const thrown = describe(event.reason);
    fail('Unhandled rejection: ' + thrown.message, thrown);
  });

  post({ type: 'ready' });
})(globalThis);
