// Pictures: an entry's (blob ids, fetched once with `blob`, kept as data URLs) and the ones
// you attach (read, made small enough to send, base64).
import { command } from "./api";

const fetched = new Map<string, Promise<string>>();

/** A blob (a PNG loom keeps) as a data URL for <img>. */
export function blobUrl(id: string): Promise<string> {
  let url = fetched.get(id);
  if (!url) {
    url = command("blob", { id }).then((a) => `data:image/png;base64,${a.data}`);
    url.catch(() => fetched.delete(id));
    fetched.set(id, url);
  }
  return url;
}

/** The longest side a picture is sent at: enough for Claude to read, small to send. */
const SIDE = 2000;

/** A picture to send: base64, scaled down to SIDE (JPEG) when bigger, else as it is. */
export async function prepare(file: Blob): Promise<string> {
  const image = await createImageBitmap(file);
  const scale = Math.min(1, SIDE / Math.max(image.width, image.height));
  if (scale === 1 && file.size < 3_000_000) return base64(file);
  const canvas = new OffscreenCanvas(Math.round(image.width * scale), Math.round(image.height * scale));
  canvas.getContext("2d")!.drawImage(image, 0, 0, canvas.width, canvas.height);
  return base64(await canvas.convertToBlob({ type: "image/jpeg", quality: 0.9 }));
}

async function base64(blob: Blob): Promise<string> {
  const bytes = new Uint8Array(await blob.arrayBuffer());
  let raw = "";
  for (let i = 0; i < bytes.length; i += 0x8000) raw += String.fromCharCode(...bytes.subarray(i, i + 0x8000));
  return btoa(raw);
}
