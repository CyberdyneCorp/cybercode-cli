#!/usr/bin/env node
/** Time the benchmark workload: node bench/bench.js [players] [ops] */
import { Leaderboard } from "../src/leaderboard.js";
import { runWorkload } from "./workload.js";

const players = Number(process.argv[2] ?? 100_000);
const ops = Number(process.argv[3] ?? 200_000);
const started = performance.now();
const checksum = runWorkload(new Leaderboard(), { players, ops });
const seconds = (performance.now() - started) / 1000;
console.log(`${players} players, ${ops} operations: ${seconds.toFixed(2)} s (checksum ${checksum})`);
