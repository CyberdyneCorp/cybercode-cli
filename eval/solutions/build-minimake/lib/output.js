'use strict';
// Synchronous writes keep minimake's messages in order with the output of the commands it runs.
const fs = require('node:fs');

const out = (line) => fs.writeSync(1, `${line}\n`);
const err = (line) => fs.writeSync(2, `${line}\n`);

module.exports = { out, err };
