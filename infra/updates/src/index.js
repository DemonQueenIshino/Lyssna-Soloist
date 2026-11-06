// @ts-check

const artifact = /^releases\/\d+\.\d+\.\d+\/windows-x86_64\/(lyssna\.exe|spotiflex\.exe|source\.zip)$/;
// Copies from before the rename read the old channel and only accept spotiflex.exe.
const channels = new Map([
  ["channels/stable/lyssna-windows-x86_64.json", "lyssna.exe"],
  ["channels/stable/windows-x86_64.json", "spotiflex.exe"],
]);

/** @param {string} value */
function decode(value) {
  return Uint8Array.from(atob(value), (character) => character.charCodeAt(0));
}

/** @param {Request} request @param {string} token */
async function authorized(request, token) {
  const supplied = request.headers.get("Authorization")?.replace(/^Bearer /, "") ?? "";
  if (!token || supplied.length > 256) return false;
  const encoder = new TextEncoder();
  const [expected, actual] = await Promise.all([
    crypto.subtle.digest("SHA-256", encoder.encode(token)),
    crypto.subtle.digest("SHA-256", encoder.encode(supplied)),
  ]);
  return crypto.subtle.timingSafeEqual(expected, actual);
}

/** @param {Request} request @param {Env} env @param {string} key */
async function publish(request, env, key) {
  if (!(await authorized(request, env.PUBLISH_TOKEN))) return new Response("Unauthorized", { status: 401 });
  const size = Number(request.headers.get("Content-Length"));
  const executable = channels.get(key);
  const manifest = executable !== undefined;
  if (!Number.isSafeInteger(size) || size <= 0 || size > (manifest ? 65536 : 512 * 1024 * 1024)) {
    return new Response("Invalid content length", { status: 413 });
  }
  if (!request.body) return new Response("Body required", { status: 400 });
  if (manifest) {
    try {
      const text = await request.text();
      if (new TextEncoder().encode(text).length > 65536) return new Response("Manifest too large", { status: 413 });
      const envelope = JSON.parse(text);
      const payload = decode(envelope.payload);
      const verificationKey = await crypto.subtle.importKey("raw", decode(env.UPDATE_PUBLIC_KEY), "Ed25519", false, ["verify"]);
      if (!(await crypto.subtle.verify("Ed25519", verificationKey, decode(envelope.signature), payload))) return new Response("Invalid signature", { status: 400 });
      const release = JSON.parse(new TextDecoder().decode(payload));
      if (!/^\d+\.\d+\.\d+$/.test(release.version) || release.target !== "windows-x86_64" || release.artifact !== `releases/${release.version}/windows-x86_64/${executable}` || !/^[a-fA-F0-9]{64}$/.test(release.sha256)) return new Response("Invalid release", { status: 400 });
      const object = await env.RELEASES.head(release.artifact);
      if (!object || object.size !== release.size) return new Response("Upload executable first", { status: 409 });
      await env.RELEASES.put(key, text, { httpMetadata: { contentType: "application/json", cacheControl: "public, max-age=60" } });
    } catch {
      return new Response("Invalid signed manifest", { status: 400 });
    }
  } else {
    if (await env.RELEASES.head(key)) return new Response("Version already exists", { status: 409 });
    const stored = await env.RELEASES.put(key, request.body, { onlyIf: { etagDoesNotMatch: "*" }, httpMetadata: { contentType: key.endsWith(".zip") ? "application/zip" : "application/octet-stream", cacheControl: "public, max-age=31536000, immutable" } });
    if (!stored) return new Response("Version already exists", { status: 409 });
  }
  return new Response(null, { status: 201 });
}

export default {
  /** @param {Request} request @param {Env} env */
  async fetch(request, env) {
    const url = new URL(request.url);
    if (url.pathname === "/health" && request.method === "GET") return Response.json({ service: "spotiflex-updates", status: "ok" });
    const publishing = url.pathname.startsWith("/publish/");
    const key = url.pathname.slice(publishing ? "/publish/".length : 1);
    if (!artifact.test(key) && !channels.has(key)) return new Response("Not found", { status: 404 });
    if (publishing) {
      if (request.method !== "PUT") return new Response("Method not allowed", { status: 405, headers: { Allow: "PUT" } });
      return publish(request, env, key);
    }
    if (!["GET", "HEAD"].includes(request.method)) return new Response("Method not allowed", { status: 405, headers: { Allow: "GET, HEAD" } });
    const object = request.method === "HEAD" ? await env.RELEASES.head(key) : await env.RELEASES.get(key);
    if (!object) return new Response("Not found", { status: 404, headers: { "Cache-Control": "no-store" } });
    const headers = new Headers({ "X-Content-Type-Options": "nosniff", "Content-Security-Policy": "default-src 'none'" });
    object.writeHttpMetadata(headers);
    headers.set("ETag", object.httpEtag);
    headers.set("Content-Length", String(object.size));
    headers.set("Cache-Control", channels.has(key) ? "public, max-age=60" : "public, max-age=31536000, immutable");
    if (request.headers.get("If-None-Match") === object.httpEtag) return new Response(null, { status: 304, headers });
    return new Response("body" in object && object.body instanceof ReadableStream ? object.body : null, { headers });
  },
};
