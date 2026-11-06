import assert from "node:assert/strict";
import { generateKeyPairSync, sign, timingSafeEqual } from "node:crypto";
import { test } from "node:test";
import worker from "../src/index.js";

Object.defineProperty(crypto.subtle, "timingSafeEqual", { value: (a, b) => timingSafeEqual(Buffer.from(a), Buffer.from(b)) });

function setup() {
  const objects = new Map();
  const keys = generateKeyPairSync("ed25519");
  const publicKey = keys.publicKey.export({ type: "spki", format: "der" }).subarray(-32).toString("base64");
  const metadata = (entry) => ({ size: entry.bytes.length, httpEtag: '"etag"', writeHttpMetadata(headers) { headers.set("Content-Type", entry.type); } });
  const env = {
    PUBLISH_TOKEN: "test-publisher-token",
    UPDATE_PUBLIC_KEY: publicKey,
    RELEASES: {
      async head(key) { const entry = objects.get(key); return entry ? metadata(entry) : null; },
      async get(key) { const entry = objects.get(key); return entry ? { ...metadata(entry), body: new Response(entry.bytes).body } : null; },
      async put(key, value, options) {
        if (options?.onlyIf && objects.has(key)) return null;
        const bytes = new Uint8Array(await new Response(value).arrayBuffer());
        objects.set(key, { bytes, type: options.httpMetadata.contentType });
        return metadata(objects.get(key));
      },
    },
  };
  const request = (path, options) => worker.fetch(new Request(`https://updates.example.com/${path}`, options), env);
  const put = (key, body, token = env.PUBLISH_TOKEN) => request(`publish/${key}`, { method: "PUT", headers: { Authorization: `Bearer ${token}`, "Content-Length": String(new TextEncoder().encode(body).length) }, body });
  const envelope = (manifest) => { const payload = Buffer.from(JSON.stringify(manifest)); return JSON.stringify({ payload: payload.toString("base64"), signature: sign(null, payload, keys.privateKey).toString("base64") }); };
  return { objects, request, put, envelope };
}

test("public service restricts paths and methods", async () => {
  const { request } = setup();
  assert.equal((await request("health")).status, 200);
  assert.equal((await request("other-private-file")).status, 404);
  assert.equal((await request("channels/stable/windows-x86_64.json")).status, 404);
  assert.equal((await request("channels/stable/windows-x86_64.json", { method: "DELETE" })).status, 405);
});

test("release uploads require authorization and cannot overwrite versioned binaries", async () => {
  const { request, put } = setup();
  const artifact = "releases/0.2.0/windows-x86_64/spotiflex.exe";
  assert.equal((await put(artifact, "binary", "wrong-token")).status, 401);
  assert.equal((await put(artifact, "binary")).status, 201);
  assert.equal((await put(artifact, "different")).status, 409);
  const response = await request(artifact);
  assert.equal(await response.text(), "binary");
  assert.equal(response.headers.get("Cache-Control"), "public, max-age=31536000, immutable");
  assert.equal((await request(artifact, { headers: { "If-None-Match": '"etag"' } })).status, 304);
});

test("only a signed manifest with an uploaded artifact can activate the channel", async () => {
  const { put, request, envelope } = setup();
  const artifact = "releases/0.2.0/windows-x86_64/spotiflex.exe";
  const channel = "channels/stable/windows-x86_64.json";
  const release = { version: "0.2.0", target: "windows-x86_64", artifact, size: 6, sha256: "a".repeat(64), notes: "Test release" };
  const signed = envelope(release);
  assert.equal((await put(channel, signed)).status, 409);
  await put(artifact, "binary");
  const tampered = JSON.parse(signed);
  tampered.payload = Buffer.from("tampered").toString("base64");
  assert.equal((await put(channel, JSON.stringify(tampered))).status, 400);
  assert.equal((await put(channel, signed)).status, 201);
  const response = await request(channel);
  assert.equal(await response.text(), signed);
  assert.equal(response.headers.get("Cache-Control"), "public, max-age=60");
});

test("each channel only activates its own executable name", async () => {
  const { put, envelope } = setup();
  const lyssna = "releases/0.3.1/windows-x86_64/lyssna.exe";
  const legacy = "releases/0.3.1/windows-x86_64/spotiflex.exe";
  await put(lyssna, "binary");
  await put(legacy, "binary");
  const release = (artifact) => envelope({ version: "0.3.1", target: "windows-x86_64", artifact, size: 6, sha256: "a".repeat(64), notes: "Test release" });
  assert.equal((await put("channels/stable/lyssna-windows-x86_64.json", release(legacy))).status, 400);
  assert.equal((await put("channels/stable/windows-x86_64.json", release(lyssna))).status, 400);
  assert.equal((await put("channels/stable/lyssna-windows-x86_64.json", release(lyssna))).status, 201);
  assert.equal((await put("channels/stable/windows-x86_64.json", release(legacy))).status, 201);
});
