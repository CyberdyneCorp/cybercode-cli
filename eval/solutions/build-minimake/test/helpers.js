'use strict';
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');
const { spawnSync } = require('node:child_process');

const MINIMAKE = path.join(__dirname, '..', 'minimake.js');
const T0 = 1_600_000_000;

/** A temporary project directory with helpers to write files and run minimake in it. */
function project(t, files = {}) {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'minimake-'));
  t.after(() => fs.rmSync(dir, { recursive: true, force: true }));
  const write = (name, text = '', mtime) => {
    const file = path.join(dir, name);
    fs.mkdirSync(path.dirname(file), { recursive: true });
    fs.writeFileSync(file, text);
    if (mtime !== undefined) fs.utimesSync(file, mtime, mtime);
  };
  for (const [name, text] of Object.entries(files)) write(name, text);
  const run = (...args) => {
    const result = spawnSync('node', [MINIMAKE, ...args], { cwd: dir, encoding: 'utf8' });
    return { out: result.stdout, err: result.stderr, code: result.status };
  };
  return { dir, write, run, exists: (name) => fs.existsSync(path.join(dir, name)) };
}

module.exports = { project, T0 };
