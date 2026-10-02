#!/usr/bin/env node
'use strict';

// Runs the lsf binary npm installed for this platform, passing everything through.
//
// The binary lives in a package of its own, `local-storefront-<platform>-<arch>`, listed in
// the optional dependencies of this one: npm installs the one that matches the machine.

const { spawn } = require('node:child_process');

const SUPPORTED = ['darwin-arm64', 'darwin-x64', 'linux-arm64', 'linux-x64', 'win32-arm64', 'win32-x64'];

const platform = `${process.platform}-${process.arch}`;
const name = `local-storefront-${platform}`;
const file = process.platform === 'win32' ? 'lsf.exe' : 'lsf';

let binary;
try {
  binary = require.resolve(`${name}/bin/${file}`);
} catch {
  const reason = SUPPORTED.includes(platform)
    ? `The package ${name} is not installed. It is an optional dependency: reinstall without\n` +
      '--omit=optional (npm) or --no-optional (yarn, pnpm).'
    : `There is no build for ${platform}. Supported: ${SUPPORTED.join(', ')}.`;
  console.error(`local-storefront: cannot find the lsf binary.\n${reason}`);
  process.exit(1);
}

const child = spawn(binary, process.argv.slice(2), { stdio: 'inherit' });

// A runner that stops this process (Playwright's webServer, a CI step) must stop the server.
const signals = ['SIGINT', 'SIGTERM', 'SIGHUP'];
const forward = (signal) => child.kill(signal);
for (const signal of signals) process.on(signal, forward);

child.on('error', (error) => {
  console.error(`local-storefront: cannot run ${binary}: ${error.message}`);
  process.exit(1);
});

child.on('exit', (code, signal) => {
  if (signal) {
    // End the same way the binary did, so that the caller sees the signal.
    for (const name of signals) process.removeListener(name, forward);
    process.kill(process.pid, signal);
  } else {
    process.exit(code ?? 1);
  }
});
