// The live reload script, run in a page just big enough for it: sockets that the test opens,
// closes and speaks through, a server to ask, and timers the test fires itself.

import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { test } from 'node:test';
import { fileURLToPath } from 'node:url';
import vm from 'node:vm';

const source = readFileSync(
  join(dirname(fileURLToPath(import.meta.url)), '..', 'live-reload.js'),
  'utf8',
);

/** Lets the promises of the script settle. */
const settle = () => new Promise((resolve) => setImmediate(resolve));

function page({ sockets = true, protocol = 'http:', answers = [] } = {}) {
  const state = { sockets: [], asked: [], timers: [], reloads: 0 };

  class WebSocket {
    constructor(url) {
      if (sockets === 'refused') throw new Error('sockets are not allowed here');
      this.url = url;
      state.sockets.push(this);
    }
    open() {
      this.onopen?.();
    }
    say(data) {
      this.onmessage?.({ data });
    }
    close() {
      this.onclose?.();
    }
  }

  const fetch = async (url) => {
    state.asked.push(url);
    const answer = answers.shift();
    if (answer === undefined) throw new TypeError('fetch failed');
    if (typeof answer === 'number') return { ok: false, status: answer, text: async () => '' };
    return { ok: true, status: 200, text: async () => answer };
  };

  vm.runInNewContext(source, {
    ...(sockets ? { WebSocket } : {}),
    fetch,
    setTimeout: (callback, delay) => state.timers.push({ callback, delay }),
    location: { protocol, host: 'shop.test:9292', reload: () => state.reloads++ },
  });

  /** Fires the timer the script is waiting on, and says how long it was set for. */
  state.wait = async () => {
    const { callback, delay } = state.timers.shift();
    callback();
    await settle();
    return delay;
  };
  return state;
}

test('the page listens on a socket and reloads when the files change', async () => {
  const state = page();
  const [socket] = state.sockets;
  assert.equal(socket.url, 'ws://shop.test:9292/__lsf/livereload');
  socket.open();
  socket.say('100');
  socket.say('100');
  assert.equal(state.reloads, 0);
  socket.say('101');
  assert.equal(state.reloads, 1);

  // The socket is enough: the server is never asked, and nothing is waited for.
  await settle();
  assert.deepEqual(state.asked, []);
  assert.deepEqual(state.timers, []);
});

test('a page served over https listens on a secure socket', () => {
  const state = page({ protocol: 'https:' });
  assert.equal(state.sockets[0].url, 'wss://shop.test:9292/__lsf/livereload');
});

test('a socket that never opens leaves the page asking the server', async () => {
  const state = page({ answers: ['100', '100', '101'] });
  state.sockets[0].close();
  await settle();
  assert.deepEqual(state.asked, ['/__lsf/livereload']);
  assert.equal(state.reloads, 0);

  assert.equal(await state.wait(), 700);
  assert.equal(state.reloads, 0);
  assert.equal(await state.wait(), 700);
  assert.equal(state.reloads, 1);
  // The page is going away: it asks no more, and tries no other socket.
  assert.deepEqual(state.timers, []);
  assert.equal(state.sockets.length, 1);
});

test('a socket that worked and closes is opened again, without asking the server', async () => {
  const state = page();
  state.sockets[0].open();
  state.sockets[0].say('100');
  // The server stops.
  state.sockets[0].close();
  assert.equal(await state.wait(), 1000);
  assert.equal(state.sockets.length, 2);
  // It is still down: the new socket closes without opening.
  state.sockets[1].close();
  assert.equal(await state.wait(), 1000);
  assert.equal(state.sockets.length, 3);
  assert.deepEqual(state.asked, []);

  // It is back, with files that changed meanwhile.
  state.sockets[2].open();
  state.sockets[2].say('101');
  assert.equal(state.reloads, 1);
});

test('the same files after a restart do not reload the page', async () => {
  const state = page();
  state.sockets[0].open();
  state.sockets[0].say('100');
  state.sockets[0].close();
  await state.wait();
  state.sockets[1].open();
  state.sockets[1].say('100');
  assert.equal(state.reloads, 0);
});

test('without sockets, or where they are refused, the page asks the server', async () => {
  for (const sockets of [false, 'refused']) {
    const state = page({ sockets, answers: ['100', '101'] });
    await settle();
    assert.deepEqual(state.sockets, []);
    assert.deepEqual(state.asked, ['/__lsf/livereload']);
    await state.wait();
    assert.equal(state.reloads, 1);
  }
});

test('a server that does not answer, or answers with an error, is asked again later', async () => {
  // No answer, an error page, then the token twice.
  const state = page({ sockets: false, answers: [undefined, 502, '100', '100'] });
  await settle();
  // Nothing is taken for the token of the files: the page does not reload when it comes.
  assert.equal(await state.wait(), 2000);
  assert.equal(await state.wait(), 2000);
  assert.equal(await state.wait(), 700);
  assert.equal(state.reloads, 0);
  assert.equal(state.asked.length, 4);
});
