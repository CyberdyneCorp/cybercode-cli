import { strict as assert } from "node:assert";
import { test } from "node:test";
import { chooseTransport, loopbackUrl, NOT_LOOPBACK } from "../../src/extension/transport.js";

test("Socket preferred: macOS and Linux use the registered socket", () => {
  const registration = { url: "http://127.0.0.1:4747", socket: "/home/u/.local/state/cyber/cyber.sock" };
  for (const platform of ["linux", "darwin"] as const) {
    assert.deepEqual(chooseTransport(registration, platform), { kind: "socket", path: registration.socket });
  }
  assert.deepEqual(chooseTransport(registration, "win32"), { kind: "tcp", url: "http://127.0.0.1:4747" });
});

test("TCP only to a loopback literal", () => {
  assert.equal(loopbackUrl("http://127.0.0.1:4747"), "http://127.0.0.1:4747");
  assert.equal(loopbackUrl("http://[::1]:4747/"), "http://[::1]:4747");
  assert.equal(loopbackUrl("http://localhost:4747"), "http://localhost:4747");
  for (const url of ["https://127.0.0.1:4747", "http://127.0.0.2:4747", "http://127.0.0.1", "http://user:pw@127.0.0.1:1", "http://127.0.0.1:1/api", "http://localhost.evil:1", "http://[::2]:1", "nonsense", undefined]) {
    assert.equal(loopbackUrl(url), undefined, String(url));
  }
});

test("LAN registration refused, and a missing registration is refused", () => {
  assert.deepEqual(chooseTransport({ url: "https://dashboard.example:4747" }, "linux"), { kind: "refused", message: NOT_LOOPBACK });
  assert.deepEqual(chooseTransport({ url: "http://192.168.1.20:4747" }, "darwin"), { kind: "refused", message: NOT_LOOPBACK });
  assert.deepEqual(chooseTransport({}, "linux"), { kind: "refused", message: NOT_LOOPBACK });
  assert.deepEqual(chooseTransport({ url: "http://127.0.0.1:1", socket: "relative.sock" }, "linux"), { kind: "tcp", url: "http://127.0.0.1:1" });
});
