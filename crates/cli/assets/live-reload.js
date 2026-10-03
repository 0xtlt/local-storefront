// Reloads the page when the theme or the store data change. The server says so over a
// WebSocket. Where a socket cannot be opened (a proxy that does not carry them, a browser
// without them), the page asks the server instead, every 700 ms.
(function () {
  var ENDPOINT = '/__lsf/livereload';
  // What the files were when the page started listening: anything else means they changed.
  var token = null;
  // Whether a socket ever opened. One that closes after that is a server that restarts,
  // not a network that refuses sockets.
  var connected = false;

  /** Takes the token the server gives. Returns whether the page is being reloaded. */
  function changed(next) {
    if (token !== null && next !== token) {
      location.reload();
      return true;
    }
    token = next;
    return false;
  }

  function poll() {
    fetch(ENDPOINT)
      .then(function (response) {
        if (!response.ok) throw new Error('live reload: ' + response.status);
        return response.text();
      })
      .then(function (next) {
        if (!changed(next)) setTimeout(poll, 700);
      })
      .catch(function () {
        setTimeout(poll, 2000);
      });
  }

  function listen() {
    var socket;
    try {
      var scheme = location.protocol === 'https:' ? 'wss://' : 'ws://';
      socket = new WebSocket(scheme + location.host + ENDPOINT);
    } catch (error) {
      poll();
      return;
    }
    socket.onopen = function () {
      connected = true;
    };
    socket.onmessage = function (event) {
      changed(event.data);
    };
    socket.onclose = function () {
      if (connected) setTimeout(listen, 1000);
      else poll();
    };
  }

  if (typeof WebSocket === 'function') listen();
  else poll();
})();
