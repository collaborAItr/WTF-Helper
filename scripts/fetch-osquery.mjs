#!/usr/bin/env node
// Downloads the pinned osquery release, checks its SHA-256, and places the binary
// where Tauri bundles it (src-tauri/binaries/osquery-<target triple>).
// Usage: node scripts/fetch-osquery.mjs [--platform mac|windows]

import { createHash } from 'node:crypto';
import { execFileSync } from 'node:child_process';
import { copyFileSync, existsSync, linkSync, mkdirSync, readdirSync, readFileSync, rmSync, statSync, writeFileSync, chmodSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const VERSION = '5.23.1';
const ASSETS = {
  mac: {
    file: `osqueryd-macos-bare-${VERSION}.tar.gz`,
    sha256: '8c10f293def8cda9734a4d8e734fac8d271032868f14c7bc4fcef789832602b6',
    binary: 'osqueryd',
    targets: ['universal-apple-darwin', 'x86_64-apple-darwin', 'aarch64-apple-darwin'],
  },
  windows: {
    file: `osquery-${VERSION}.windows_x86_64.zip`,
    sha256: '7bd411050ef6b5aae1b23956aec0dc5ce6e800c5656f0cd463ac70a6e1bdf30b',
    binary: 'osqueryd.exe',
    targets: ['x86_64-pc-windows-msvc'],
  },
};

const root = dirname(dirname(fileURLToPath(import.meta.url)));
const cacheDir = join(root, '.cache', 'osquery');
const binDir = join(root, 'src-tauri', 'binaries');

const flag = process.argv.indexOf('--platform');
const platform = flag >= 0 ? process.argv[flag + 1] : process.platform === 'win32' ? 'windows' : 'mac';
const asset = ASSETS[platform];
if (!asset) {
  console.error(`Unknown platform "${platform}". Use mac or windows.`);
  process.exit(1);
}

const sha256 = (path) => createHash('sha256').update(readFileSync(path)).digest('hex');

function find(dir, name) {
  for (const entry of readdirSync(dir)) {
    const path = join(dir, entry);
    if (statSync(path).isDirectory()) {
      const hit = find(path, name);
      if (hit) return hit;
    } else if (entry.toLowerCase() === name.toLowerCase()) {
      return path;
    }
  }
  return null;
}

mkdirSync(cacheDir, { recursive: true });
mkdirSync(binDir, { recursive: true });
const archive = join(cacheDir, asset.file);

if (!existsSync(archive) || sha256(archive) !== asset.sha256) {
  const url = `https://github.com/osquery/osquery/releases/download/${VERSION}/${asset.file}`;
  console.log(`Downloading ${url}`);
  const response = await fetch(url);
  if (!response.ok) throw new Error(`Download failed: ${response.status}`);
  writeFileSync(archive, Buffer.from(await response.arrayBuffer()));
}
const actual = sha256(archive);
if (actual !== asset.sha256) {
  rmSync(archive, { force: true });
  throw new Error(`SHA-256 mismatch for ${asset.file}: expected ${asset.sha256}, got ${actual}`);
}

const extractDir = join(cacheDir, `${platform}-extract`);
rmSync(extractDir, { recursive: true, force: true });
mkdirSync(extractDir, { recursive: true });
// bsdtar reads both .tar.gz and .zip, and ships with macOS and Windows 10+.
// On Windows, name it directly so Git's GNU tar (which cannot read zip) is never picked up.
const tar = process.platform === 'win32' ? join(process.env.SystemRoot || 'C:\\Windows', 'System32', 'tar.exe') : 'tar';
execFileSync(tar, ['-xf', archive, '-C', extractDir], { stdio: 'inherit' });
const binary = find(extractDir, asset.binary);
if (!binary) throw new Error(`${asset.binary} not found in ${asset.file}`);

const ext = platform === 'windows' ? '.exe' : '';
const [first, ...rest] = asset.targets;
const primary = join(binDir, `osquery-${first}${ext}`);
rmSync(primary, { force: true });
copyFileSync(binary, primary);
chmodSync(primary, 0o755);
for (const target of rest) {
  const path = join(binDir, `osquery-${target}${ext}`);
  rmSync(path, { force: true });
  linkSync(primary, path);
}
rmSync(extractDir, { recursive: true, force: true });
console.log(`osquery ${VERSION} ready: ${asset.targets.map((t) => `osquery-${t}${ext}`).join(', ')}`);
