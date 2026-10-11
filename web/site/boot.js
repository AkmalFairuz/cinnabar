// Cinnabar's browser launcher: signs in on the web relay, gathers the game files, then starts the
// WebAssembly client. The relay serves this page, so its WebSockets share the page's origin.

// Panics report their whole stack, not the browser's default ten frames.
Error.stackTraceLimit = 100;

const CONTROL_PROTOCOL = "cinnabar.control.v1";
const SESSION_PREFIX = "sid.";
const SESSION_KEY = "cinnabar.relay-session";
const PHYSICS_REGISTRY = "block-physics-v2193.bin";
const DEV_ASSETS = "v1/dev-assets/";
const DEV_ASSETS_INDEX = `${DEV_ASSETS}index.json`;
// Sound bank indexes kept across visits, by the hash that ends each.
const SOUND_INDEX_CACHE = "cinnabar-sound-index";
// The client module and game files kept between visits. Each is revalidated with its ETag, so a
// repeat Play reuses unchanged files instead of downloading them again.
const ASSET_CACHE = "cinnabar-assets";

import { acceptDroppedPacks, installPackStore } from "./packs.js";

// The client downloads only once the player presses Play.
let clientLoad = null;
let loadedClient = null;
// The first join's startup: whether it runs, whether the client draws yet, and the server's
// resource packs downloading meanwhile, which the overlay shows until they finish.
const firstJoin = { running: false, drawing: false, packs: null };

/** Downloads and starts the client module once, reporting its bytes to `onProgress`. */
function loadClient(onProgress) {
  clientLoad ??= import("./pkg/web_client.js")
    .then(async (client) => {
      const exports = await client.default({ module_or_path: await clientModule(onProgress) });
      // The client's heap in bytes, for checking its memory from the console.
      globalThis.cinnabarHeapBytes = () => exports.memory.buffer.byteLength;
      // Global Resources keep imported packs in this page's storage.
      installPackStore(client.resource_pack_extensions());
      loadedClient = client;
      return client;
    })
    .catch((error) => {
      clientLoad = null;
      throw error;
    });
  return clientLoad;
}

const $ = (id) => document.getElementById(id);
const relayUrl = `${location.protocol === "https:" ? "wss" : "ws"}://${location.host}`;

let sessionId = null;
let assetFiles = null;
// Byte sizes of served carriers, from the relay's index; picked files carry their own.
let servedSizes = new Map();

// wgpu requests no inter-stage variable limit, so devices get WebGPU's default of 16, below what
// the terrain and model shaders pass. Ask for the adapter's own limit until wgpu maps it.
if (globalThis.GPUAdapter) {
  const requestDevice = GPUAdapter.prototype.requestDevice;
  GPUAdapter.prototype.requestDevice = function (descriptor = {}) {
    const requiredLimits = { ...(descriptor.requiredLimits || {}) };
    requiredLimits.maxInterStageShaderVariables ??= this.limits.maxInterStageShaderVariables;
    return requestDevice.call(this, { ...descriptor, requiredLimits });
  };
}

// Browsers expose WebGPU only to secure pages, so a plain-HTTP LAN address hides it.
if (!navigator.gpu) {
  $("support").hidden = false;
  $("support-reason").textContent = window.isSecureContext
    ? "This browser has no WebGPU. Use a current Chrome, Edge or Safari."
    : `WebGPU needs a secure page: open this over https:// or from localhost (this is ${location.origin}).`;
}

function readStoredSession() {
  try {
    return sessionStorage.getItem(SESSION_KEY);
  } catch {
    return null;
  }
}

function storeSession(id) {
  try {
    sessionStorage.setItem(SESSION_KEY, id);
  } catch {
    // Without storage a reload starts a new session.
  }
}

function showAccount(account) {
  $("device-code").hidden = account.state !== "awaiting_code";
  $("sign-in").hidden = account.state === "signed_in";
  $("sign-out").hidden = account.state !== "signed_in";
  $("sign-in").disabled = account.state === "awaiting_code";
  switch (account.state) {
    case "signed_in":
      $("account-state").textContent = account.gamertag
        ? `Signed in as ${account.gamertag}.`
        : "Signed in.";
      break;
    case "awaiting_code":
      $("account-state").textContent = "Waiting for you to approve the sign-in…";
      $("verification-link").href = account.verification_uri;
      $("verification-link").textContent = account.verification_uri;
      $("user-code").textContent = account.user_code;
      break;
    case "failed":
      $("account-state").textContent = account.reason || "Sign-in failed.";
      break;
    default:
      $("account-state").textContent = "Signed out. Servers in online mode need a Microsoft account.";
  }
}

function connectControl() {
  const stored = readStoredSession();
  const protocols = stored ? [CONTROL_PROTOCOL, SESSION_PREFIX + stored] : [CONTROL_PROTOCOL];
  const socket = new WebSocket(`${relayUrl}/v1/control`, protocols);
  socket.onmessage = (message) => {
    const event = JSON.parse(message.data);
    if (event.type === "session") {
      sessionId = event.id;
      storeSession(event.id);
      $("sign-in").disabled = false;
      updatePlay();
    } else if (event.type === "account") {
      showAccount(event.account);
    } else if (event.type === "connect") {
      // The relay prepares the join, so its pack downloads reach the join screen from here.
      loadedClient?.set_connect_progress(JSON.stringify(event.connect ?? {}));
      showPackDownload(event.connect ?? {});
    } else if (event.type === "error") {
      $("account-state").textContent = `The relay refused that (${event.error}).`;
    }
  };
  socket.onclose = () => {
    $("account-state").textContent = "Lost the relay; reconnecting…";
    $("sign-in").disabled = true;
    setTimeout(connectControl, 2000);
  };
  $("sign-in").onclick = () => socket.send(JSON.stringify({ op: "sign_in" }));
  $("sign-out").onclick = () => socket.send(JSON.stringify({ op: "sign_out" }));
}

/** The chosen files below the compiled-assets folder, keyed by their path inside it. */
function compiledCarriers(files) {
  const carriers = new Map();
  for (const file of files) {
    const parts = file.webkitRelativePath.split("/");
    const relative = parts.slice(1).join("/");
    if (relative) {
      carriers.set(relative, file);
    }
  }
  return carriers;
}

const REQUIRED_CARRIERS = ["vanilla-v2193.mcbea", "vanilla-v1.mcbeui", "vanilla-v1.mcbeent"];

/** Accepts `carriers` (relative path -> File or served URL) once the required ones are present. */
function useCarriers(carriers, description) {
  const missing = REQUIRED_CARRIERS.filter((name) => !carriers.has(name));
  if (missing.length) {
    $("assets-state").textContent = `The game files are missing ${missing.join(", ")}.`;
    assetFiles = null;
  } else {
    assetFiles = carriers;
    $("assets-state").textContent = description;
  }
  updatePlay();
}

$("assets").onchange = (event) => {
  const carriers = compiledCarriers(event.target.files);
  useCarriers(carriers, `${carriers.size} game files ready.`);
};

/** Uses the carriers a development relay serves (`-dev-assets`), so nothing needs importing. */
async function useServedCarriers() {
  let index;
  try {
    const response = await fetch(DEV_ASSETS_INDEX, { cache: "no-store" });
    if (!response.ok) {
      return;
    }
    index = await response.json();
  } catch {
    return;
  }
  const carriers = new Map(index.files.map(({ path }) => [path, `${DEV_ASSETS}${path}`]));
  servedSizes = new Map(index.files.map(({ path, size }) => [path, size]));
  $("assets-picker").hidden = true;
  useCarriers(carriers, "The relay provides the game files; they download when you press Play.");
}

/** Bytes `first..=last` of `url`, which must answer the range. */
async function fetchRange(url, first, last) {
  const response = await fetch(url, { headers: { Range: `bytes=${first}-${last}` } });
  if (response.status !== 206) {
    throw new Error(`${url} does not serve byte ranges (${response.status})`);
  }
  return new Uint8Array(await response.arrayBuffer());
}

/** Registers the relay's sound bank by its index alone; the client downloads each sound later. */
async function addRangedSoundBank(client, carriers, relative, source) {
  const url = new URL(source, location.href).href;
  carriers.add_ranged_compiled(relative, await soundBankPrefix(client, url), url);
}

/**
 * The index prefix of the sound bank at `url`. Its last bytes hash the rest, so a copy kept under
 * that hash stands in for the download until the bank changes.
 */
async function soundBankPrefix(client, url) {
  const header = await fetchRange(url, 0, client.sound_bank_header_bytes() - 1);
  const length = client.sound_bank_prefix_length(header);
  const hash = await fetchRange(url, length - client.sound_bank_hash_bytes(), length - 1);
  const key = `${url}?index=${Array.from(hash, (byte) => byte.toString(16).padStart(2, "0")).join("")}`;
  // Cache Storage is missing outside secure pages and may refuse writes when full.
  const cache = await globalThis.caches?.open(SOUND_INDEX_CACHE).catch(() => null);
  const kept = await cache?.match(key).catch(() => null);
  if (kept) {
    const prefix = new Uint8Array(await kept.arrayBuffer());
    if (prefix.length === length && client.sound_bank_prefix_valid(prefix)) {
      return prefix;
    }
  }
  const prefix = await fetchRange(url, 0, length - 1);
  if (cache) {
    // A recompiled bank replaces the index kept for the one before.
    for (const request of await cache.keys().catch(() => [])) {
      if (request.url.startsWith(`${url}?index=`) && request.url !== key) {
        await cache.delete(request).catch(() => {});
      }
    }
    await cache.put(key, new Response(prefix)).catch(() => {});
  }
  return prefix;
}

/** The bytes of one carrier, from a chosen file or the relay. */
/**
 * The bytes of a picked file or served URL, reporting the bytes read so far to `onProgress` as
 * they arrive. A download streams into one buffer of the `expected` size when it is known.
 */
async function carrierBytes(source, expected, onProgress) {
  if (source instanceof File) {
    const bytes = new Uint8Array(await source.arrayBuffer());
    onProgress(bytes.length);
    return bytes;
  }
  const { cache, kept, response } = await revalidated(source);
  if (kept) {
    const bytes = new Uint8Array(await kept.arrayBuffer());
    onProgress(bytes.length);
    return bytes;
  }
  if (!response.ok) {
    throw new Error(`could not download ${source} (${response.status})`);
  }
  if (!response.body) {
    const bytes = new Uint8Array(await response.arrayBuffer());
    onProgress(bytes.length);
    await keep(cache, source, response, bytes);
    return bytes;
  }
  const bytes = await streamedBytes(response, expected, onProgress);
  await keep(cache, source, response, bytes);
  return bytes;
}

/**
 * The body of `response`, reporting the bytes read so far to `onProgress` as they arrive, read
 * into one buffer of the `expected` size when it is known.
 */
async function streamedBytes(response, expected, onProgress) {
  const reader = response.body.getReader();
  let buffer = new Uint8Array(expected || decodedLength(response));
  let received = 0;
  let shown = 0;
  for (;;) {
    const { done, value } = await reader.read();
    if (done) {
      break;
    }
    if (received + value.length > buffer.length) {
      const grown = new Uint8Array(Math.max(buffer.length * 2, received + value.length));
      grown.set(buffer.subarray(0, received));
      buffer = grown;
    }
    buffer.set(value, received);
    received += value.length;
    if (performance.now() - shown > 50) {
      shown = performance.now();
      onProgress(received);
    }
  }
  onProgress(received);
  return received === buffer.length ? buffer : buffer.slice(0, received);
}

/**
 * The client module as a Response the browser compiles while it streams: this browser's kept
 * copy when the relay confirms it is current, else a download that is kept for the next visit,
 * its bytes received and total (0 when unknown) reported to `onProgress`.
 */
async function clientModule(onProgress) {
  const url = "pkg/web_client_bg.wasm";
  const { cache, kept, response } = await revalidated(url);
  if (kept) {
    return kept;
  }
  if (!response.ok || !response.body) {
    return response;
  }
  const total = decodedLength(response);
  // A copy is kept only where it can be stored; an unread branch would buffer the whole module.
  const keeping = cache && response.headers.get("etag");
  const [compiled, stored] = keeping ? response.body.tee() : [response.body, null];
  if (stored) {
    keep(cache, url, response, stored);
  }
  let received = 0;
  let shown = 0;
  const counted = compiled.pipeThrough(
    new TransformStream({
      transform(chunk, controller) {
        received += chunk.length;
        if (performance.now() - shown > 50) {
          shown = performance.now();
          onProgress(received, Math.max(total, received));
        }
        controller.enqueue(chunk);
      },
      flush() {
        onProgress(received, Math.max(total, received));
      },
    }),
  );
  return new Response(counted, { headers: { "Content-Type": "application/wasm" } });
}

/**
 * Fetches `url` past the HTTP cache, asking the relay whether this browser's kept copy is still
 * current: `kept` is that copy on a 304, else `response` is the download.
 */
async function revalidated(url) {
  const cache = await globalThis.caches?.open(ASSET_CACHE).catch(() => null);
  const kept = await cache?.match(url).catch(() => null);
  const etag = kept?.headers.get("etag");
  const response = await fetch(url, { cache: "no-store", headers: etag ? { "If-None-Match": etag } : {} });
  return response.status === 304 && kept ? { cache, kept } : { cache, response };
}

/**
 * Keeps `body` downloaded with `response` under `url` with its ETag, for revalidation on the
 * next visit; a missing ETag, an unavailable cache or a full quota only costs a later download.
 */
async function keep(cache, url, response, body) {
  const etag = response.headers.get("etag");
  if (!cache || !etag) {
    return;
  }
  const headers = { etag, "Content-Type": response.headers.get("content-type") ?? "application/octet-stream" };
  await cache.put(url, new Response(body, { headers })).catch(() => {});
}

/**
 * The decoded size of `response`'s body, or 0 when unknown. A compressed response's
 * Content-Length is its compressed size, so the relay names the decoded size separately.
 */
function decodedLength(response) {
  const decoded = Number(response.headers.get("x-decoded-length"));
  if (decoded) {
    return decoded;
  }
  return response.headers.get("content-encoding") ? 0 : Number(response.headers.get("content-length")) || 0;
}

/**
 * Shows the first join's resource pack download over the startup overlay while it runs, and
 * hides the overlay once the download ends after the game has started drawing. Later joins show
 * it on the game's own join screen.
 */
function showPackDownload(connect) {
  if (!firstJoin.running) {
    return;
  }
  const received = connect.received_bytes ?? 0;
  const total = connect.total_bytes ?? 0;
  if (connect.stage === "packs" && received < total) {
    firstJoin.packs = connect;
    showStartup(`Downloading resource packs ${megabytes(received)} / ${megabytes(total)} MB…`, received / total);
  } else if (firstJoin.packs) {
    firstJoin.packs = null;
    if (firstJoin.drawing) {
      firstJoin.running = false;
      $("startup").hidden = true;
    }
  }
}

/** `bytes` in megabytes with one decimal, as progress labels show them. */
function megabytes(bytes) {
  return (bytes / 1e6).toFixed(1);
}

// The last server and name, so returning from a session finds them filled in.
const REMEMBERED = { server: "cinnabar.server", "display-name": "cinnabar.displayName" };
for (const [field, key] of Object.entries(REMEMBERED)) {
  try {
    const value = localStorage.getItem(key);
    if (value) {
      $(field).value = value;
    }
  } catch {
    // Storage is unavailable in some private windows; the fields start empty.
  }
}

function rememberFields() {
  for (const [field, key] of Object.entries(REMEMBERED)) {
    try {
      localStorage.setItem(key, $(field).value.trim());
    } catch {
      // Nothing to keep without storage.
    }
  }
}

$("server").oninput = updatePlay;

// What the player agreed to, as the desktop's first run keeps it: new consent text or a new EULA
// link asks again.
const EULA_KEY = "cinnabar.eulaAccepted";
const eulaConsent = () => `${$("eula-text").textContent.replace(/\s+/g, " ").trim()}\n${$("eula-link").href}`;
try {
  $("eula-accept").checked = localStorage.getItem(EULA_KEY) === eulaConsent();
} catch {
  // Without storage the player accepts again on each visit.
}
$("eula-accept").onchange = () => {
  try {
    if ($("eula-accept").checked) {
      localStorage.setItem(EULA_KEY, eulaConsent());
    } else {
      localStorage.removeItem(EULA_KEY);
    }
  } catch {
    // Nothing to keep without storage.
  }
  updatePlay();
};

function updatePlay() {
  $("play").disabled = !(
    sessionId && assetFiles && $("server").value.trim() && navigator.gpu && $("eula-accept").checked
  );
}

/** Shows startup `label` over the page, with `fraction` of the bar filled when known. */
function showStartup(label, fraction) {
  $("startup").hidden = false;
  $("startup-label").textContent = label;
  if (fraction !== null && fraction !== undefined) {
    $("startup-bar").style.width = `${Math.round(fraction * 100)}%`;
  }
}

/** Resolves once the page has painted, so a label set before a long task is on screen. */
function painted() {
  return new Promise((resolve) => requestAnimationFrame(() => setTimeout(resolve)));
}

$("play").onclick = async () => {
  // No Mojang-derived file downloads before the player accepts the EULA.
  if (!$("eula-accept").checked) {
    return;
  }
  $("play").disabled = true;
  $("play-state").textContent = "";
  rememberFields();
  try {
    showStartup("Downloading the game…", 0);
    const client = await loadClient((received, total) =>
      showStartup(`Downloading the game ${megabytes(received)} / ${megabytes(total)} MB…`, total ? received / total : null),
    );
    // Compile reports and the prepare stamp are never read, so they are not downloaded.
    const wanted = [...assetFiles].filter(([relative]) => !client.carrier_unused(relative));
    const soundBank = client.sound_bank_file();
    const languages = `${client.language_carrier_dir()}/`;
    const carriers = new client.Carriers();
    // Whole carriers download now; the sound bank and languages are read later by URL.
    const later = (relative, source) =>
      (relative === soundBank && typeof source === "string") || relative.startsWith(languages);
    const sizeOf = (relative, source) => (source instanceof File ? source.size : servedSizes.get(relative) ?? 0);
    const total = wanted
      .filter(([relative, source]) => !later(relative, source))
      .reduce((sum, [relative, source]) => sum + sizeOf(relative, source), 0);
    const verb = wanted.some(([, source]) => typeof source === "string") ? "Downloading" : "Loading";
    let done = 0;
    const report = (bytes) =>
      showStartup(
        `${verb} game files ${megabytes(done + bytes)} / ${megabytes(total)} MB…`,
        total ? Math.min((done + bytes) / total, 1) : null,
      );
    for (const [relative, source] of wanted) {
      if (relative === soundBank && typeof source === "string") {
        await addRangedSoundBank(client, carriers, relative, source);
      } else if (relative.startsWith(languages)) {
        // A language downloads only once a player picks it.
        const url = typeof source === "string" ? new URL(source, location.href).href : URL.createObjectURL(source);
        carriers.add_remote_compiled(relative, url);
      } else {
        const bytes = await carrierBytes(source, sizeOf(relative, source), report);
        done += bytes.length;
        carriers.add_compiled(relative, bytes);
      }
    }
    const physics = await fetch(PHYSICS_REGISTRY);
    if (!physics.ok) {
      throw new Error(`could not load ${PHYSICS_REGISTRY}`);
    }
    carriers.set_physics_registry(new Uint8Array(await physics.arrayBuffer()));
    $("launcher").hidden = true;
    showStartup("Preparing the game…", 0);
    await painted();
    firstJoin.running = true;
    const progress = (label, fraction, ready) => {
      firstJoin.drawing ||= ready;
      if (firstJoin.packs) {
        return;
      }
      if (ready) {
        firstJoin.running = false;
        $("startup").hidden = true;
      } else {
        showStartup(`${label}…`, fraction);
      }
    };
    client.start(carriers, relayUrl, sessionId, $("server").value.trim(), $("display-name").value.trim() || "Steve", progress);
    // A pack dropped on the game imports into Global Resources.
    acceptDroppedPacks(client);
  } catch (error) {
    firstJoin.running = false;
    firstJoin.packs = null;
    $("startup").hidden = true;
    $("launcher").hidden = false;
    $("play-state").textContent = String(error);
    updatePlay();
  }
};

connectControl();
useServedCarriers();
