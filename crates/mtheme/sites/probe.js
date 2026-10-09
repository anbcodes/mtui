// Paste into the browser console (F12) on the site you want to theme, signed in
// and on the screen you care about. It copies a compact report to the clipboard:
// how the site themes itself (root classes, custom properties) and which colours
// it really uses where. Send the report back.
(() => {
  const rgb = (c) => { const m = c.match(/[\d.]+/g); return m && m.length >= 3 && (m[3] === undefined || +m[3] > 0.02) ? '#' + m.slice(0, 3).map((x) => (+x | 0).toString(16).padStart(2, '0')).join('') + (m[3] !== undefined && +m[3] < 1 ? '/' + (+m[3]).toFixed(2) : '') : null; };
  const sel = (e) => { let s = e.tagName.toLowerCase(); if (e.id) s += '#' + e.id; const c = [...e.classList].filter((x) => !/^(u|is|has)-/.test(x)).slice(0, 2); if (c.length) s += '.' + c.join('.'); return s; };
  const out = { url: location.origin, title: document.title };
  const root = document.documentElement;
  out.rootClass = root.className;
  out.rootAttrs = Object.fromEntries([...root.attributes].filter((a) => a.name !== 'class').map((a) => [a.name, a.value.slice(0, 60)]));
  out.bodyClass = document.body.className.slice(0, 120);
  out.colorScheme = getComputedStyle(root).colorScheme;
  // custom properties, grouped by the selector that defines them
  const vars = {};
  const walk = (rules) => { for (const r of rules) { if (r.cssRules && !r.style) { walk(r.cssRules); continue; } if (!r.style) continue; const names = [...r.style].filter((p) => p.startsWith('--')); if (names.length && /^(:root|html|body|\.|\[)/.test(r.selectorText || '')) { const k = r.selectorText.slice(0, 80); (vars[k] ||= []).push(...names); } } };
  for (const s of document.styleSheets) { try { walk(s.cssRules); } catch (e) { out.crossOriginSheets = (out.crossOriginSheets || 0) + 1; } }
  out.varGroups = Object.entries(vars).filter(([, v]) => v.length >= 3).sort((a, b) => b[1].length - a[1].length).slice(0, 8).map(([k, v]) => ({ selector: k, count: v.length, sample: v.slice(0, 40).map((n) => n + '=' + getComputedStyle(root).getPropertyValue(n).trim().slice(0, 30)) }));
  // which colours are used where, weighted by area
  const bg = new Map(), fg = new Map();
  const add = (m, c, e, w) => { if (!c) return; const o = m.get(c) || { w: 0, ex: new Set() }; o.w += w; if (o.ex.size < 3) o.ex.add(sel(e)); m.set(c, o); };
  for (const e of document.querySelectorAll('body *')) {
    const r = e.getBoundingClientRect(); if (r.width < 4 || r.height < 4) continue;
    const cs = getComputedStyle(e);
    add(bg, rgb(cs.backgroundColor), e, r.width * r.height);
    if (e.childNodes.length && [...e.childNodes].some((n) => n.nodeType === 3 && n.textContent.trim())) add(fg, rgb(cs.color), e, 1 + r.width);
  }
  const top = (m, n) => [...m.entries()].sort((a, b) => b[1].w - a[1].w).slice(0, n).map(([c, o]) => c + '  ' + [...o.ex].join(' '));
  out.backgrounds = top(bg, 14);
  out.textColors = top(fg, 10);
  const anchors = {};
  for (const q of ['body', 'header', 'nav', 'aside', 'main', 'button', 'input', 'a', '[role=button]', '[role=listbox] > *', '[aria-selected=true]']) { const e = document.querySelector(q); if (e) { const cs = getComputedStyle(e); anchors[q] = [rgb(cs.backgroundColor), rgb(cs.color), rgb(cs.borderTopColor)].join(' '); } }
  out.anchors = anchors;
  const text = JSON.stringify(out, null, 1);
  try { copy(text); console.log('mtheme probe copied to the clipboard (' + text.length + ' chars)'); } catch (e) { console.log(text); }
  return text.length;
})();
