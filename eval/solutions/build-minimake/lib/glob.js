'use strict';
// $(wildcard): per-component globbing with *, ? and [...] sets.
const fs = require('node:fs');

const MAGIC = /[*?[]/;

function componentRegex(component) {
  let source = '';
  for (let i = 0; i < component.length; i++) {
    const c = component[i];
    if (c === '*') source += '[^/]*';
    else if (c === '?') source += '[^/]';
    else if (c === '[' && component.indexOf(']', i + 2) > 0) {
      const end = component.indexOf(']', i + 2);
      let set = component.slice(i + 1, end);
      const negate = set.startsWith('!');
      if (negate) set = set.slice(1);
      source += `[${negate ? '^' : ''}${set.replace(/[\\\]^]/g, '\\$&')}]`;
      i = end;
    } else source += c.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
  }
  return new RegExp(`^${source}$`);
}

const joinPath = (dir, name) => (dir === '' ? name : dir.endsWith('/') ? dir + name : `${dir}/${name}`);

function listDir(dir) {
  try {
    return fs.readdirSync(dir === '' ? '.' : dir);
  } catch {
    return [];
  }
}

function glob(pattern) {
  const absolute = pattern.startsWith('/');
  const components = (absolute ? pattern.slice(1) : pattern).split('/');
  let paths = [absolute ? '/' : ''];
  for (const component of components) {
    if (!MAGIC.test(component)) {
      paths = paths.map((p) => joinPath(p, component));
      continue;
    }
    const regex = componentRegex(component);
    const hiddenOk = component.startsWith('.');
    paths = paths.flatMap((dir) => listDir(dir)
      .filter((name) => regex.test(name) && (hiddenOk || !name.startsWith('.')))
      .map((name) => joinPath(dir, name)));
  }
  return paths.filter((p) => fs.existsSync(p)).sort((a, b) => (a < b ? -1 : a > b ? 1 : 0));
}

module.exports = { glob };
