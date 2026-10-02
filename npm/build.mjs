#!/usr/bin/env node
// Assembles the npm packages from the archives of a release.
//
//   node npm/build.mjs --artifacts dist --out npm-dist --version 0.1.0
//
// `--artifacts` holds the archives the release workflow builds (`lsf-<target>.tar.gz|zip`).
// The result is one directory per package, ready for `npm publish`:
//
//   npm-dist/local-storefront-<platform>-<arch>/   the binary of one platform
//   npm-dist/local-storefront/                     the launcher, which depends on all of them

import { execFileSync } from 'node:child_process';
import { chmodSync, cpSync, existsSync, mkdirSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { parseArgs } from 'node:util';

// One package per platform npm can tell apart with `os` and `cpu`. Linux gets the static
// (musl) build: it runs on every distribution, Alpine included, and is as fast as the glibc one.
const PLATFORMS = [
  { os: 'darwin', cpu: 'arm64', target: 'aarch64-apple-darwin', label: 'macOS on Apple silicon' },
  { os: 'darwin', cpu: 'x64', target: 'x86_64-apple-darwin', label: 'macOS on Intel' },
  { os: 'linux', cpu: 'arm64', target: 'aarch64-unknown-linux-musl', label: 'Linux on ARM64' },
  { os: 'linux', cpu: 'x64', target: 'x86_64-unknown-linux-musl', label: 'Linux on x86-64' },
  { os: 'win32', cpu: 'arm64', target: 'aarch64-pc-windows-msvc', label: 'Windows on ARM64' },
  { os: 'win32', cpu: 'x64', target: 'x86_64-pc-windows-msvc', label: 'Windows on x86-64' },
];

const { values: options } = parseArgs({
  options: {
    artifacts: { type: 'string' },
    out: { type: 'string' },
    version: { type: 'string' },
  },
});
for (const name of ['artifacts', 'out', 'version']) {
  if (!options[name]) {
    console.error(`usage: node npm/build.mjs --artifacts <dir> --out <dir> --version <version>`);
    process.exit(2);
  }
}
if (!/^\d+\.\d+\.\d+(-[0-9A-Za-z.-]+)?$/.test(options.version)) {
  console.error(`"${options.version}" is not a version (expected 1.2.3 or 1.2.3-rc.1)`);
  process.exit(2);
}

const here = dirname(fileURLToPath(import.meta.url));
const launcher = join(here, 'local-storefront');
const manifest = JSON.parse(readFileSync(join(launcher, 'package.json'), 'utf8'));

rmSync(options.out, { recursive: true, force: true });
mkdirSync(options.out, { recursive: true });

const write = (directory, name, content) => {
  mkdirSync(directory, { recursive: true });
  writeFileSync(join(directory, name), content);
};

const optionalDependencies = {};
for (const { os, cpu, target, label } of PLATFORMS) {
  const name = `${manifest.name}-${os}-${cpu}`;
  const windows = os === 'win32';
  const archive = join(options.artifacts, `lsf-${target}.${windows ? 'zip' : 'tar.gz'}`);
  if (!existsSync(archive)) {
    console.error(`missing ${archive}`);
    process.exit(1);
  }

  const directory = join(options.out, name);
  const bin = join(directory, 'bin');
  mkdirSync(bin, { recursive: true });
  if (windows) {
    execFileSync('unzip', ['-q', '-o', archive, 'lsf.exe', '-d', bin]);
  } else {
    execFileSync('tar', ['-xzf', archive, '-C', bin, 'lsf']);
    chmodSync(join(bin, 'lsf'), 0o755);
  }

  const file = windows ? 'lsf.exe' : 'lsf';
  write(
    directory,
    'package.json',
    JSON.stringify(
      {
        name,
        version: options.version,
        description: `The lsf binary of ${manifest.name} for ${label}.`,
        license: manifest.license,
        homepage: manifest.homepage,
        bugs: manifest.bugs,
        repository: manifest.repository,
        os: [os],
        cpu: [cpu],
        files: [`bin/${file}`],
        // Yarn Plug'n'Play must leave the binary on disk to be executable.
        preferUnplugged: true,
      },
      null,
      2,
    ) + '\n',
  );
  write(
    directory,
    'README.md',
    `# ${name}\n\nThe \`lsf\` binary of [${manifest.name}](https://www.npmjs.com/package/${manifest.name}) for ${label}.\nInstall \`${manifest.name}\` instead: it picks the package that matches your machine.\n`,
  );
  optionalDependencies[name] = options.version;
  console.log(`${name}@${options.version}  <-  ${archive}`);
}

// The launcher depends on exactly this version of every platform package.
const directory = join(options.out, manifest.name);
cpSync(launcher, directory, { recursive: true });
write(
  directory,
  'package.json',
  JSON.stringify({ ...manifest, version: options.version, optionalDependencies }, null, 2) + '\n',
);
console.log(`${manifest.name}@${options.version}`);
