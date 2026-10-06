// The HTML page around the rendered markdown: screen and print styles, an
// outline, and (for the live preview) the script that reloads and scrolls.

use crate::md::{esc, Doc};

const CSS: &str = r##"
:root{color-scheme:light dark;--bg:#fff;--fg:#1f2328;--dim:#656d76;--line:#d8dee4;--code-bg:#f6f8fa;--link:#0969da;--accent:#0969da;
--k1:#cf222e;--k2:#8250df;--k3:#6639ba;--k4:#0a3069;--k5:#0550ae;--k6:#6e7781;--k7:#953800;--k8:#0550ae;
--note:#0969da;--tip:#1a7f37;--important:#8250df;--warning:#9a6700;--caution:#cf222e}
@media (prefers-color-scheme:dark){:root{--bg:#0d1117;--fg:#e6edf3;--dim:#8d96a0;--line:#30363d;--code-bg:#161b22;--link:#4493f8;--accent:#4493f8;
--k1:#ff7b72;--k2:#ffa657;--k3:#d2a8ff;--k4:#a5d6ff;--k5:#79c0ff;--k6:#8b949e;--k7:#ffa657;--k8:#79c0ff;
--note:#4493f8;--tip:#3fb950;--important:#ab7df8;--warning:#d29922;--caution:#f85149}}
*{box-sizing:border-box}
html{scroll-padding-top:1rem}
body{margin:0;background:var(--bg);color:var(--fg);font:16px/1.65 system-ui,-apple-system,"Segoe UI",Roboto,sans-serif;-webkit-text-size-adjust:100%}
#layout{display:flex;justify-content:center;gap:3rem;padding:2.5rem 1.5rem 5rem}
main{min-width:0;max-width:46rem;width:100%}
nav#toc{display:none;position:sticky;top:2rem;align-self:flex-start;width:15rem;max-height:calc(100vh - 4rem);overflow:auto;font-size:.85rem;line-height:1.4;color:var(--dim)}
nav#toc a{display:block;color:inherit;text-decoration:none;padding:.18rem 0}
nav#toc a:hover{color:var(--link)}
nav#toc .l2{padding-left:.9rem}nav#toc .l3{padding-left:1.8rem}
@media (min-width:1150px){nav#toc.has{display:block}}
h1,h2,h3,h4,h5,h6{line-height:1.25;margin:1.8em 0 .6em;font-weight:650;position:relative}
h1{font-size:2em;padding-bottom:.3em;border-bottom:1px solid var(--line);margin-top:0}
h2{font-size:1.5em;padding-bottom:.25em;border-bottom:1px solid var(--line)}
h3{font-size:1.2em}h4{font-size:1em}h5{font-size:.9em}h6{font-size:.85em;color:var(--dim)}
a.anchor{position:absolute;left:-1.2em;width:1.2em;color:var(--dim);text-decoration:none;opacity:0;font-weight:400}
h1:hover a.anchor,h2:hover a.anchor,h3:hover a.anchor,h4:hover a.anchor,h5:hover a.anchor,h6:hover a.anchor{opacity:1}
p,ul,ol,blockquote,pre,table,.alert,.html{margin:0 0 1em}
a{color:var(--link);text-decoration:none}a:hover{text-decoration:underline}
img{max-width:100%;height:auto}
ul,ol{padding-left:1.6em}li>ul,li>ol{margin:.25em 0}li+li{margin-top:.2em}
ul.tasks{list-style:none;padding-left:.2em}ul.tasks ul{padding-left:1.6em}
li input[type=checkbox]{margin:0 .45em .1em -.2em;vertical-align:middle}
blockquote{margin-left:0;padding:0 1em;color:var(--dim);border-left:.25em solid var(--line)}
blockquote>:last-child{margin-bottom:0}
code,pre{font:.875em/1.5 ui-monospace,SFMono-Regular,Menlo,Consolas,monospace}
code{background:var(--code-bg);padding:.15em .35em;border-radius:4px}
pre{background:var(--code-bg);padding:1em 1.1em;border-radius:6px;overflow:auto;position:relative}
pre code{background:none;padding:0;font-size:1em}
pre[data-lang]::before{content:attr(data-lang);position:absolute;top:.3rem;right:.6rem;font-size:.7rem;color:var(--dim);text-transform:uppercase;letter-spacing:.05em}
table{border-collapse:collapse;display:block;overflow:auto;max-width:100%}
th,td{border:1px solid var(--line);padding:.35em .8em}th{background:var(--code-bg)}tr:nth-child(2n) td{background:color-mix(in srgb,var(--code-bg) 55%,transparent)}
hr{border:0;border-top:1px solid var(--line);margin:2em 0}
del{color:var(--dim)}
kbd{font:.8em ui-monospace,monospace;border:1px solid var(--line);border-bottom-width:2px;border-radius:4px;padding:.1em .4em;background:var(--code-bg)}
details{margin:0 0 1em}summary{cursor:pointer}
.alert{border-left:.25em solid var(--c);padding:.2em 1em;--c:var(--note)}
.alert-tip{--c:var(--tip)}.alert-important{--c:var(--important)}.alert-warning{--c:var(--warning)}.alert-caution{--c:var(--caution)}
.alert-title{color:var(--c);font-weight:600;margin:.4em 0}
.k1{color:var(--k1)}.k2{color:var(--k2)}.k3{color:var(--k3)}.k4{color:var(--k4)}.k5{color:var(--k5)}.k6{color:var(--k6);font-style:italic}.k7{color:var(--k7)}.k8{color:var(--k8);font-weight:600}
#live{position:fixed;right:.8rem;bottom:.7rem;font:12px system-ui,sans-serif;color:var(--dim);background:var(--bg);border:1px solid var(--line);border-radius:99px;padding:.2rem .7rem;cursor:pointer;user-select:none;opacity:.85}
#live.off b{color:#d1242f}#live b{color:#1a7f37}
@media print{
:root{color-scheme:light;--bg:#fff;--fg:#000;--dim:#444;--line:#bbb;--code-bg:#f3f3f3;--link:#000;
--k1:#a00;--k2:#609;--k3:#518;--k4:#064;--k5:#036;--k6:#555;--k7:#730;--k8:#036}
@page{margin:2cm 2.2cm}
html,body{background:#fff}
body{font:11pt/1.5 Georgia,"Times New Roman",serif;-webkit-print-color-adjust:exact;print-color-adjust:exact}
#layout{display:block;padding:0}main{max-width:none}
nav#toc,#live,a.anchor{display:none!important}
h1,h2,h3,h4,h5,h6{break-after:avoid;page-break-after:avoid}
pre,blockquote,table,img,.alert,tr,li>input{break-inside:avoid;page-break-inside:avoid}
pre{white-space:pre-wrap;word-break:break-word;border:1px solid var(--line);background:#fafafa}
code,pre{font-size:9pt}
p,li{orphans:3;widows:3}
a{text-decoration:underline}
a[href^="http"]:not([href*="#"])::after{content:" (" attr(href) ")";font-size:.82em;color:var(--dim);word-break:break-all}
h1{font-size:22pt}h2{font-size:16pt}h3{font-size:13pt}
}
"##;

const JS: &str = r##"
(() => {
  const doc = () => document.getElementById('doc');
  const pill = document.getElementById('live');
  let sync = true, cur = null;
  const marks = () => [...doc().querySelectorAll('[data-line]')].map(e => [+e.dataset.line, e]);
  const top = e => e.getBoundingClientRect().top + scrollY;
  function scrollToLine(n) {
    const L = marks(); if (!L.length) return;
    let a = null, b = null;
    for (const x of L) { if (x[0] <= n) a = x; else { b = x; break; } }
    let y = 0;
    if (a && b) y = top(a[1]) + (top(b[1]) - top(a[1])) * (n - a[0]) / Math.max(1, b[0] - a[0]);
    else if (a) y = top(a[1]);
    scrollTo(0, Math.max(0, y - innerHeight * 0.3));
  }
  const label = () => { pill.innerHTML = (pill.classList.contains('off') ? '<b>○</b> offline' : '<b>●</b> live') + ' · sync ' + (sync ? 'on' : 'off'); };
  pill.onclick = () => { sync = !sync; label(); if (sync && cur != null) scrollToLine(cur); };
  label();
  const v = document.querySelector('meta[name=v]').content;
  const es = new EventSource('/events?v=' + v);
  es.onopen = () => { pill.classList.remove('off'); label(); };
  es.onerror = () => { pill.classList.add('off'); label(); };
  es.addEventListener('reload', async () => {
    const t = await (await fetch('/', { cache: 'no-store' })).text();
    const d = new DOMParser().parseFromString(t, 'text/html');
    const y = scrollY;
    doc().innerHTML = d.getElementById('doc').innerHTML;
    document.getElementById('toc').innerHTML = d.getElementById('toc').innerHTML;
    document.getElementById('toc').className = d.getElementById('toc').className;
    document.title = d.title;
    scrollTo(0, y);
    if (sync && cur != null) scrollToLine(cur);
  });
  es.addEventListener('scroll', e => { cur = +e.data; if (sync) scrollToLine(cur); });
  addEventListener('load', () => cur != null && sync && scrollToLine(cur), true);
  doc().addEventListener('dblclick', e => {
    const t = e.target.closest('[data-line]');
    if (t) fetch('/goto?line=' + t.dataset.line, { method: 'POST' });
  });
})();
"##;

/// A complete HTML document. With `live`, it includes the script that talks
/// to the preview server (event stream at `/events`, `version` tells the
/// server which render this page is).
pub fn page(doc: &Doc, title: &str, live: bool, version: u64) -> String {
    let toc: String = doc.headings.iter().filter(|h| h.level <= 3).map(|h| format!("<a class=\"l{}\" href=\"#{}\">{}</a>", h.level, h.id, esc(&h.text))).collect();
    let has = doc.headings.iter().filter(|h| h.level <= 3).count() > 2;
    format!(
        "<!doctype html>\n<html lang=\"en\"><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width,initial-scale=1\"><meta name=\"v\" content=\"{}\"><title>{}</title><style>{}</style></head>\n<body><div id=\"layout\"><main id=\"doc\">\n{}</main><nav id=\"toc\" class=\"{}\">{}</nav></div>\n{}</body></html>\n",
        version,
        esc(title),
        CSS,
        doc.html,
        if has { "has" } else { "" },
        toc,
        if live { format!("<div id=\"live\"></div><script>{}</script>\n", JS) } else { String::new() }
    )
}
