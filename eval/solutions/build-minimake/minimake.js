#!/usr/bin/env node
'use strict';
// minimake: a small make-like build tool. See SPEC.md for the contract.
const fs = require('node:fs');
const { Builder } = require('./lib/builder');
const { MakeError } = require('./lib/errors');
const { Expander } = require('./lib/expander');
const { err } = require('./lib/output');
const { parseMakefile } = require('./lib/parser');
const { Variables } = require('./lib/variables');

class UsageError extends Error {}

const FLAGS = { n: 'dryRun', k: 'keepGoing', B: 'alwaysMake' };
const VALUE_OPTIONS = { f: 'files', C: 'directories' };

/** Parse the command line (SPEC §1). */
function parseArguments(argv) {
  const options = { dryRun: false, keepGoing: false, alwaysMake: false, files: [], directories: [] };
  const assignments = [];
  const goals = [];
  for (let i = 0; i < argv.length; i++) {
    const arg = argv[i];
    if (arg.startsWith('--')) throw new UsageError(`unknown option '${arg}'`);
    if (arg.startsWith('-') && arg.length > 1) {
      for (let j = 1; j < arg.length; j++) {
        const letter = arg[j];
        if (letter in FLAGS) {
          options[FLAGS[letter]] = true;
        } else if (letter in VALUE_OPTIONS) {
          const value = j + 1 < arg.length ? arg.slice(j + 1) : argv[++i];
          if (value === undefined) throw new UsageError(`option '-${letter}' requires an argument`);
          options[VALUE_OPTIONS[letter]].push(value);
          break;
        } else {
          throw new UsageError(`unknown option '-${letter}'`);
        }
      }
    } else if (arg.includes('=')) {
      assignments.push([arg.slice(0, arg.indexOf('=')), arg.slice(arg.indexOf('=') + 1)]);
    } else {
      goals.push(arg);
    }
  }
  return { options, assignments, goals };
}

function readMakefile(files) {
  const name = files.at(-1) ?? 'Makefile';
  try {
    return { name, text: fs.readFileSync(name, 'utf8') };
  } catch {
    throw new MakeError(files.length ? `*** cannot read makefile '${name}'.  Stop.` : '*** No makefile found.  Stop.');
  }
}

function main(argv) {
  const { options, assignments, goals } = parseArguments(argv);
  for (const dir of options.directories) {
    try {
      process.chdir(dir);
    } catch {
      throw new MakeError(`*** cannot change to directory '${dir}'.  Stop.`);
    }
  }
  const vars = new Variables();
  for (const [name, value] of assignments) vars.defineCommandLine(name, value);
  const expander = new Expander(vars);
  const { name, text } = readMakefile(options.files);
  const makefile = parseMakefile(name, text, vars, expander);
  if (goals.length === 0) {
    if (makefile.defaultGoal === null) throw new MakeError('*** No targets.  Stop.');
    goals.push(makefile.defaultGoal);
  }
  return new Builder(makefile, vars, expander, options).run(goals);
}

try {
  process.exitCode = main(process.argv.slice(2));
} catch (error) {
  if (!(error instanceof MakeError || error instanceof UsageError)) throw error;
  err(`minimake: ${error.message}`);
  process.exitCode = 2;
}
