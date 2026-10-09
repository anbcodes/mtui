// @domain duckduckgo.com
// mtheme: DuckDuckGo hard-codes white on pieces whose class names change between builds (the Search
// Assist card, buttons, suggestion lists). This re-colours any near-white surface and dark-on-dark text
// with the theme's colours, which duckduckgo.css exposes as --mt-* variables (empty on Classic).
(() => {
  const root = document.documentElement;
  const v = (n) => getComputedStyle(root).getPropertyValue(n).trim();
  if (!v("--mt-surface")) return;
  const lum = (c) => {
    const m = c && c.match(/[\d.]+/g);
    if (!m || m.length < 3 || (m[3] !== undefined && +m[3] < 0.6)) return null;
    const f = (x) => ((x /= 255) <= 0.03928 ? x / 12.92 : ((x + 0.055) / 1.055) ** 2.4);
    return 0.2126 * f(+m[0]) + 0.7152 * f(+m[1]) + 0.0722 * f(+m[2]);
  };
  const dark = lum(v("--mt-base")) < 0.4;
  const style = document.createElement("style");
  style.textContent = '[data-mt-fixed], [data-mt-fixed] *:not(a):not(a *) { color: var(--mt-text) !important; } [data-mt-fixed] a { color: var(--mt-link, inherit) !important; } [data-mt-pseudo]::before, [data-mt-pseudo]::after { background-color: var(--mt-surface) !important; background-image: none !important; }';
  document.head.appendChild(style);
  const light = (c) => { const l = lum(c); return l !== null && (dark ? l > 0.55 : l < 0.2) && c !== "rgba(0, 0, 0, 0)"; };
  const fix = (el) => {
    if (el.nodeType !== 1 || /^(IMG|SVG|PATH|VIDEO|CANVAS|HTML|BODY|SCRIPT|STYLE)$/i.test(el.tagName)) return;
    const cs = getComputedStyle(el);
    const bg = lum(cs.backgroundColor);
    // a light surface on a dark theme (or the reverse), or a light-theme grey chip on a dark theme
    if (bg !== null && (dark ? bg > 0.55 : bg < 0.2) && cs.backgroundColor !== "rgba(0, 0, 0, 0)") {
      el.style.setProperty("background-color", "var(--mt-surface)", "important");
      el.setAttribute("data-mt-fixed", "");
      return;
    }
    if (light(getComputedStyle(el, "::before").backgroundColor) || light(getComputedStyle(el, "::after").backgroundColor)) el.setAttribute("data-mt-pseudo", "");
    const fg = lum(cs.color);
    if (dark && fg !== null && fg < 0.12 && (bg === null || bg < 0.3) && el.children.length === 0 && el.textContent.trim()) {
      el.style.setProperty("color", "var(--mt-text)", "important");
    }
  };
  let queued = false;
  const sweep = () => {
    queued = false;
    for (const el of document.querySelectorAll("body *")) {
      const r = el.getBoundingClientRect();
      if (r.width >= 8 && r.height >= 8) fix(el);
    }
  };
  const later = () => { if (!queued) { queued = true; setTimeout(sweep, 250); } };
  new MutationObserver(later).observe(document.body, { childList: true, subtree: true });
  later();
})();
