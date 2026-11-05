// The agent's words as HTML: markdown (marked), then sanitized (DOMPurify), since agent
// output is untrusted; links open elsewhere, without the page as opener. Without a working
// sanitizer the text is shown escaped, never as HTML (fail closed).
import { marked } from "marked";
import createDOMPurify from "dompurify";

marked.setOptions({ gfm: true, breaks: false });

let purify: ReturnType<typeof createDOMPurify> | null = null;
function sanitizer() {
  if (!purify) {
    purify = createDOMPurify(window);
    purify.addHook("afterSanitizeAttributes", (node) => {
      if (node.tagName === "A") { node.setAttribute("target", "_blank"); node.setAttribute("rel", "noopener noreferrer"); }
    });
  }
  return purify;
}

const escape = (t: string) => t.replace(/[&<>"']/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[c]!);

export function markdown(text: string): string {
  const p = sanitizer();
  if (!p.isSupported) return `<p>${escape(text)}</p>`;
  return p.sanitize(marked.parse(text, { async: false }) as string, { FORBID_TAGS: ["style", "form", "input"], FORBID_ATTR: ["style"] });
}
