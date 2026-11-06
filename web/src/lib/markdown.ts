// The agent's words as HTML: markdown (marked), then sanitized (DOMPurify), since agent
// output is untrusted; links open elsewhere, without the page as opener. Without a working
// sanitizer the text is shown escaped, never as HTML (fail closed).
import { marked } from "marked";
import createDOMPurify from "dompurify";

marked.setOptions({ gfm: true, breaks: false });

// Code blocks coloured (highlight.js, the common languages only: the page stays small). Its
// output is escaped spans, sanitized with the rest.
import hljs from "highlight.js/lib/core";
import bash from "highlight.js/lib/languages/bash";
import css from "highlight.js/lib/languages/css";
import diff from "highlight.js/lib/languages/diff";
import go from "highlight.js/lib/languages/go";
import javascript from "highlight.js/lib/languages/javascript";
import json from "highlight.js/lib/languages/json";
import kotlin from "highlight.js/lib/languages/kotlin";
import python from "highlight.js/lib/languages/python";
import rust from "highlight.js/lib/languages/rust";
import sql from "highlight.js/lib/languages/sql";
import swift from "highlight.js/lib/languages/swift";
import typescript from "highlight.js/lib/languages/typescript";
import xml from "highlight.js/lib/languages/xml";
import yaml from "highlight.js/lib/languages/yaml";
import ini from "highlight.js/lib/languages/ini";
for (const [name, lang] of Object.entries({ bash, css, diff, go, javascript, json, kotlin, python, rust, sql, swift, typescript, xml, yaml, ini })) hljs.registerLanguage(name, lang);
hljs.registerAliases(["sh", "shell", "zsh", "console"], { languageName: "bash" });
hljs.registerAliases(["toml"], { languageName: "ini" });
hljs.registerAliases(["html", "svelte", "vue"], { languageName: "xml" });
hljs.registerAliases(["ts", "tsx"], { languageName: "typescript" });
hljs.registerAliases(["js", "jsx", "mjs"], { languageName: "javascript" });
hljs.registerAliases(["rs"], { languageName: "rust" });
hljs.registerAliases(["py"], { languageName: "python" });

marked.use({
  renderer: {
    code({ text, lang }) {
      const language = (lang ?? "").split(/\s/)[0];
      const known = language && hljs.getLanguage(language);
      const body = known ? hljs.highlight(text, { language, ignoreIllegals: true }).value : escape(text);
      return `<pre><code class="hljs${known ? ` language-${language}` : ""}">${body}</code></pre>`;
    },
  },
});

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
