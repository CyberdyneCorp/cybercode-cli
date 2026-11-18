/** Mocha entry inside the VS Code extension host. */
import { appendFileSync } from "node:fs";
import Mocha from "mocha";
import { chatSuite } from "./chat.test";
import { restrictedSuite } from "./restricted.test";

export function run(): Promise<void> {
  const mocha = new Mocha({ ui: "tdd", timeout: 60_000, color: true });
  mocha.suite.emit("pre-require", globalThis, "bundle", mocha);
  if (process.env.CYBER_TEST_SUITE === "restricted") restrictedSuite();
  else chatSuite();
  // The extension host's stdout is not always forwarded: results also go to a file the
  // launcher prints.
  const report = (line: string) => {
    if (process.env.CYBER_TEST_REPORT) appendFileSync(process.env.CYBER_TEST_REPORT, `${line}\n`);
  };
  return new Promise((resolve, reject) => {
    const runner = mocha.run((failures) => (failures > 0 ? reject(new Error(`${failures} integration tests failed`)) : resolve()));
    runner.on("pass", (test) => report(`ok - ${test.fullTitle()}`));
    runner.on("fail", (test, error: Error) => report(`not ok - ${test.fullTitle()}: ${error.message}`));
  });
}
