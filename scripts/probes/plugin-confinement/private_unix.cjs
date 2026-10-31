// CJS is required by the same untrusted Node/Bun/Deno bootstrap fixture.
const net = require("node:net");
const path = require("node:path");

function listen(server, endpoint) {
  return new Promise((resolve, reject) => {
    server.once("error", reject);
    server.listen(endpoint, resolve);
  });
}

function acknowledgement(socket) {
  return new Promise((resolve, reject) => {
    socket.once("error", reject);
    socket.once("data", data => resolve(data.length === 1 && data[0] === 65));
    socket.once("connect", () => socket.write("T"));
  });
}

async function exchange(endpoint) {
  const peers = new Set();
  const server = net.createServer(peer => {
    peers.add(peer);
    peer.on("error", () => {});
    peer.once("data", data => {
      if (data.length === 1 && data[0] === 84) peer.end("A");
      else peer.destroy();
    });
  });
  server.on("error", () => {});
  let client, timer;
  const deadline = new Promise((_, reject) => {
    timer = setTimeout(() => reject(new Error("private stream exchange timed out")), 3000);
  });
  try {
    await Promise.race([listen(server, endpoint), deadline]);
    client = new net.Socket();
    const response = acknowledgement(client);
    client.connect(endpoint);
    return await Promise.race([response, deadline]);
  } catch {
    return false;
  } finally {
    clearTimeout(timer);
    client?.destroy();
    for (const peer of peers) peer.destroy();
    await new Promise(resolve => server.close(() => resolve()));
  }
}

module.exports = async function privateUnix(scratch) {
  const absolute = await exchange(path.join(scratch, "a.sock"));
  const previous = process.cwd();
  try {
    process.chdir(scratch);
    return [absolute, await exchange("r.sock")];
  } finally {
    process.chdir(previous);
  }
};
