// loom's core API from the page: one WebSocket, JSON-RPC 2.0 (docs/api.md). Requests answer
// promises; subscriptions keep a copy of their resource (snapshot pages, `synced`, then
// changes; a gap asks for a resync) and call back whenever it changes. Commands are checked
// against the API's generated types (api-types.ts).

import { PROTOCOL as VERSION, type Commands, type Message } from "./api-types";

const PROTOCOL = { min: VERSION, max: VERSION };

/** A resource's item: every item has an id; the rest is the resource's own (api-types.ts). */
export type Item = { id: string; [field: string]: any };
type Change = { op: string; id?: string; item?: Item; field?: string; text?: string; ids?: string[]; offset?: number; data?: string };
/** A subscription delivery, as the spec defines it. */
type Delivery = Message;
export type State = "connecting" | "open" | "closed" | "refused";
type Listener = (state: State, hello: any) => void;

// The session: in the address's fragment once (sent to no server), then this tab's.
if (location.hash.length > 1) {
  sessionStorage.setItem("loom", location.hash.slice(1));
  history.replaceState(null, "", "/");
}

let socket: WebSocket | null = null;
let next = 0;
const waiting = new Map<string, { resolve: (v: any) => void; reject: (e: Error) => void }>();
const followed = new Map<string, Follow>();
const bySub = new Map<string, Follow>();
let hello: any = null;
const listeners = new Set<Listener>();
let state: State = "connecting";

/** The connection's state: connecting, open (with `hello`), closed, refused. */
export function onState(f: Listener) {
  listeners.add(f);
  f(state, hello);
  return () => listeners.delete(f);
}

function setState(s: State) {
  state = s;
  for (const f of listeners) f(state, hello);
}

function call(method: string, params: object = {}, id = String(++next)): Promise<any> {
  return new Promise((resolve, reject) => {
    if (!socket || socket.readyState !== WebSocket.OPEN) return reject(new Error("not connected to loom"));
    waiting.set(id, { resolve, reject });
    socket.send(JSON.stringify({ jsonrpc: "2.0", id, method, params }));
  });
}

/** A command (its id unique to this page, so a retry isn't done twice). */
export function command<K extends keyof Commands>(method: K, params: Commands[K]): Promise<any> {
  return call(method, params, `web-${crypto.randomUUID()}`);
}

/** Whether loom has command `name` (loomd has some the window doesn't). */
export function has(name: string): boolean {
  return !!hello?.capabilities?.commands?.includes(name);
}

export class Follow {
  items: Item[] = [];
  synced = false;
  gone = false;
  seq = 0;
  incoming: Item[] | null = null;
  stale = false;
  sub: string | null = null;
  error?: string;
  watchers = new Set<(f: Follow) => void>();
  constructor(public resource: string) {}

  async start() {
    try {
      const answer = await call("subscribe", { resource: this.resource });
      const sub: string = answer.sub;
      this.sub = sub;
      bySub.set(sub, this);
    } catch (e) {
      this.error = (e as Error).message;
      this.tell();
    }
  }

  apply(m: Delivery) {
    if (this.stale) {
      this.seq = m.seq;
      if (m.kind === "resync") { this.stale = false; this.incoming = []; }
      return;
    }
    if (this.seq !== 0 && m.seq !== this.seq + 1) {
      this.seq = m.seq;
      this.stale = true;
      this.synced = false;
      call("resync", { sub: this.sub }).catch(() => {});
      return;
    }
    this.seq = m.seq;
    switch (m.kind) {
      case "page": (this.incoming ??= []).push(...(m.items as Item[])); return;
      case "synced": this.items = this.incoming ?? []; this.incoming = null; this.synced = true; break;
      case "resync": this.synced = false; this.incoming = []; return;
      case "gone": this.gone = true; this.items = []; break;
      // a new array each time: a UI that keeps `items` sees it changed
      case "changes": { const items = [...this.items]; for (const op of m.ops) applyOp(items, op as Change); this.items = items; break; }
      default: return;
    }
    this.tell();
  }

  tell() {
    for (const f of this.watchers) f(this);
  }
}

function at(items: Item[], id: string | undefined) {
  return items.findIndex((i) => i.id === id);
}

function applyOp(items: Item[], op: Change) {
  const i = op.id !== undefined ? at(items, op.id) : op.item ? at(items, op.item.id) : -1;
  switch (op.op) {
    case "upsert": if (!op.item) break; if (i < 0) items.push(op.item); else items[i] = op.item; break;
    case "remove": if (i >= 0) items.splice(i, 1); break;
    case "append": if (i >= 0 && op.field) items[i] = { ...items[i], [op.field]: (typeof items[i][op.field] === "string" ? items[i][op.field] : "") + (op.text ?? "") }; break;
    case "order": {
      const ids = op.ids ?? [];
      const first = ids.map((id) => items.find((x) => x.id === id)).filter((x): x is Item => !!x);
      const rest = items.filter((x) => !ids.includes(x.id));
      items.splice(0, items.length, ...first, ...rest);
      break;
    }
    case "bytes": {
      // A job's output: kept as raw bytes after its screen, what the screen holds dropped.
      if (i < 0) break;
      const item = items[i];
      const have = item.offset ?? 0;
      const offset = op.offset ?? 0;
      if (offset > have) break; // missed some: the copy goes on (a resync would fix it)
      const bytes = fromBase64(op.data).slice(have - offset);
      items[i] = { ...item, offset: have + bytes.length, tail: concat(item.tail ?? new Uint8Array(), bytes) };
      break;
    }
  }
}

export function fromBase64(text: string | undefined): Uint8Array {
  const raw = atob(text || "");
  const bytes = new Uint8Array(raw.length);
  for (let i = 0; i < raw.length; i++) bytes[i] = raw.charCodeAt(i);
  return bytes;
}

function concat(a: Uint8Array, b: Uint8Array) {
  const out = new Uint8Array(a.length + b.length);
  out.set(a);
  out.set(b, a.length);
  return out;
}

/** Follows `resource`; `watch(follow)` on every change. Returns the stop. */
export function follow(resource: string, watch: (f: Follow) => void): () => void {
  let f = followed.get(resource);
  if (!f) {
    f = new Follow(resource);
    followed.set(resource, f);
    if (state === "open") f.start();
  }
  f.watchers.add(watch);
  if (f.synced || f.error) watch(f);
  return () => {
    f.watchers.delete(watch);
    if (f.watchers.size === 0) {
      followed.delete(resource);
      if (f.sub) {
        bySub.delete(f.sub);
        call("unsubscribe", { sub: f.sub }).catch(() => {});
      }
    }
  };
}

function connect() {
  const session = sessionStorage.getItem("loom");
  if (!session) return setState("refused");
  setState("connecting");
  socket = new WebSocket(`ws://${location.host}/api?s=${encodeURIComponent(session)}`);
  const open = socket;
  open.onopen = async () => {
    try {
      hello = await call("initialize", { protocol: PROTOCOL, client: { name: "loom web", version: "0" }, capabilities: {} });
    } catch {
      return setState("refused");
    }
    setState("open");
    for (const f of followed.values()) {
      Object.assign(f, { items: [], synced: false, seq: 0, incoming: null, stale: false, sub: null });
      f.start();
    }
  };
  open.onmessage = ({ data }) => {
    const m = JSON.parse(data);
    if (m.method === "subscription") return bySub.get(m.params.sub)?.apply(m.params);
    const w = waiting.get(m.id);
    if (!w) return;
    waiting.delete(m.id);
    if (m.error) w.reject(new Error(m.error.message)); else w.resolve(m.result);
  };
  open.onclose = () => {
    for (const w of waiting.values()) w.reject(new Error("loom closed the connection"));
    waiting.clear();
    bySub.clear();
    // Turned away before it opened (no session, or let go): a new link is needed.
    if (state !== "open") return setState("refused");
    setState("closed");
    setTimeout(connect, 2000);
  };
}

connect();
