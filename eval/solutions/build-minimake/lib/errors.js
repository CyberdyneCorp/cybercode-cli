'use strict';

/** A fatal error. `message` is printed after `minimake: ` and minimake exits 2. */
class MakeError extends Error {}

module.exports = { MakeError };
