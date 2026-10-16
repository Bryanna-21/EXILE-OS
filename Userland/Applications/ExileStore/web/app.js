/* Exile Store web UI. One page that adapts to phones, tablets, desktops and TVs.
 * All server data is inserted with textContent (never innerHTML), because catalog
 * metadata is untrusted. Icons are the only innerHTML and are static constants. */
(() => {
'use strict';

// ---------------------------------------------------------------- helpers
const $ = (sel, root = document) => root.querySelector(sel);

function h(tag, props, ...kids) {
  const el = document.createElement(tag);
  for (const [k, v] of Object.entries(props || {})) {
    if (v === undefined || v === null || v === false) continue;
    if (k === 'class') el.className = v;
    else if (k === 'text') el.textContent = v;
    else if (k.startsWith('on')) el.addEventListener(k.slice(2), v);
    else if (v === true) el.setAttribute(k, '');
    else el.setAttribute(k, v);
  }
  for (const kid of kids.flat()) {
    if (kid === null || kid === undefined || kid === false) continue;
    el.append(kid.nodeType ? kid : document.createTextNode(String(kid)));
  }
  return el;
}

const ICONS = {
  home: '<path d="M3 11.5 12 4l9 7.5V20a1 1 0 0 1-1 1h-5v-6H9v6H4a1 1 0 0 1-1-1z"/>',
  explore: '<circle cx="12" cy="12" r="9"/><path d="m15.5 8.5-2 5-5 2 2-5z"/>',
  search: '<circle cx="11" cy="11" r="7"/><path d="m20 20-3.5-3.5"/>',
  library: '<path d="M4 4h4v16H4zM10 4h4v16h-4z"/><path d="m16.5 5.5 3.7-1 3 14.5-3.8 1z" transform="translate(-2 0)"/>',
};
function icon(name) {
  const s = document.createElementNS('http://www.w3.org/2000/svg', 'svg');
  s.setAttribute('viewBox', '0 0 24 24');
  s.setAttribute('fill', 'none');
  s.setAttribute('stroke', 'currentColor');
  s.setAttribute('stroke-width', '2');
  s.setAttribute('stroke-linecap', 'round');
  s.setAttribute('stroke-linejoin', 'round');
  s.setAttribute('aria-hidden', 'true');
  s.innerHTML = ICONS[name];
  return s;
}

function checkMark() {
  const n = document.createElementNS('http://www.w3.org/2000/svg', 'svg');
  n.setAttribute('class', 'check-mark'); n.setAttribute('viewBox', '0 0 20 20'); n.setAttribute('aria-hidden', 'true');
  const p = document.createElementNS('http://www.w3.org/2000/svg', 'path');
  p.setAttribute('d', 'M4 10.5l4 4 8-9'); p.setAttribute('fill', 'none'); p.setAttribute('stroke', 'currentColor'); p.setAttribute('stroke-width', '2.6'); p.setAttribute('stroke-linecap', 'round'); p.setAttribute('stroke-linejoin', 'round');
  n.append(p); return n;
}

function humanSize(n) {
  if (!n) return '';
  const u = ['B', 'KB', 'MB', 'GB', 'TB'];
  let i = 0, v = n;
  while (v >= 1024 && i < u.length - 1) { v /= 1024; i++; }
  return (i === 0 ? v : v.toFixed(v < 10 ? 1 : 0)) + ' ' + u[i];
}

function fmtDate(iso) {
  const d = new Date(iso);
  if (!iso || isNaN(d)) return '';
  return d.toLocaleDateString(undefined, { year: 'numeric', month: 'short', day: 'numeric' });
}

function hashOf(s) { let x = 0; for (const c of s) x = (x * 31 + c.charCodeAt(0)) >>> 0; return x; }
function initials(name) {
  const w = name.replace(/[^\p{L}\p{N} ]/gu, '').trim().split(/\s+/).filter(Boolean);
  return ((w[0] || '?')[0] + (w.length > 1 ? w[1][0] : '')).toUpperCase();
}
function avatar(app, big) {
  return h('div', { class: 'avatar a' + (hashOf(app.appId) % 5) + (big ? ' big' : ''), 'aria-hidden': 'true', text: initials(app.name) });
}

// ---------------------------------------------------------------- state
const state = {
  token: null,
  session: null,
  tasks: [],
  seen: new Map(),         // taskId -> last known finished flag
  dismissed: new Set(),
  poll: null,
  routeSeq: 0,
  forcedUi: null,
};

function safeStorage(op, key, value) {
  try { return op === 'get' ? sessionStorage.getItem(key) : op === 'set' ? sessionStorage.setItem(key, value) : sessionStorage.removeItem(key); }
  catch (_) { return null; }
}

// ---------------------------------------------------------------- API
class ApiError extends Error { constructor(status, message) { super(message); this.status = status; } }

async function api(path, opts = {}) {
  const init = { method: opts.method || 'GET', headers: { Authorization: 'Bearer ' + state.token }, cache: 'no-store' };
  if (opts.body !== undefined) { init.headers['Content-Type'] = 'application/json'; init.body = JSON.stringify(opts.body); }
  let res;
  try { res = await fetch(path, init); }
  catch (_) { throw new ApiError(0, 'The store service on this device is not responding.'); }
  let data = null;
  try { data = await res.json(); } catch (_) { /* non-JSON */ }
  if (res.status === 401) { showNoAccess(); throw new ApiError(401, 'Not signed in'); }
  if (!res.ok) throw new ApiError(res.status, (data && data.error) || 'Something went wrong.');
  return data;
}

// ---------------------------------------------------------------- layout / input mode
const TV_UA = /(smart-?tv|googletv|android tv|aft[a-z]{1,4}\b|crkey|tizen|web0s|webos|hbbtv|bravia|roku|appletv|netcast|viera|vizio)/i;

function computeUi() {
  const cls = state.session ? state.session.device.class : '';
  const forced = state.forcedUi;
  let input;
  if (forced === 'remote' || forced === 'tv') input = 'remote';
  else if (forced === 'touch' || forced === 'pointer') input = forced;
  else if (cls === 'tv' || TV_UA.test(navigator.userAgent)) input = 'remote';
  else input = matchMedia('(pointer: fine)').matches ? 'pointer' : 'touch';
  const w = window.innerWidth;
  const layout = input === 'remote' ? 'tv' : w < 600 ? 'compact' : w < 1024 ? 'medium' : 'expanded';
  document.body.dataset.input = input;
  document.body.dataset.layout = layout;
}
window.addEventListener('resize', computeUi);

// ---------------------------------------------------------------- navigation
const NAV = [
  ['home', 'Home', 'home'],
  ['explore', 'Explore', 'explore'],
  ['search', 'Search', 'search'],
  ['library', 'Library', 'library'],
];

function renderNav(active) {
  const nav = $('#nav');
  nav.replaceChildren(h('div', { class: 'brand', 'aria-hidden': 'true' }, 'EXILE ', h('span', { text: 'STORE' })));
  for (const [id, label, ic] of NAV)
    nav.append(h('a', { href: '#/' + id, 'aria-current': active === id ? 'page' : null }, icon(ic), h('span', { text: label })));
}

function parseRoute() {
  const raw = location.hash.replace(/^#\/?/, '');
  const [pathPart, queryPart] = raw.split('?');
  const parts = pathPart.split('/').filter(Boolean).map(decodeURIComponent);
  return { name: parts[0] || 'home', arg: parts[1] || '', params: new URLSearchParams(queryPart || '') };
}

function goBack() {
  if (history.length > 1) history.back(); else location.hash = '#/home';
}

async function route() {
  if (!state.token) return;
  const seq = ++state.routeSeq;
  const r = parseRoute();
  const main = $('#main');
  const activeNav = { home: 'home', explore: 'explore', category: 'explore', search: 'search', library: 'library', app: null }[r.name];
  renderNav(r.name === 'app' ? (state.lastNav || 'home') : activeNav);
  if (activeNav) state.lastNav = activeNav;
  main.setAttribute('aria-busy', 'true');
  main.classList.remove('enter');
  main.replaceChildren(...skeleton(r.name === 'app' ? 'app' : 'list'));
  try {
    let view;
    if (r.name === 'home') view = await viewHome();
    else if (r.name === 'explore') view = await viewExplore();
    else if (r.name === 'category') view = await viewCategory(r.arg);
    else if (r.name === 'search') view = await viewSearch(r.params.get('q') || '');
    else if (r.name === 'library') view = await viewLibrary();
    else if (r.name === 'app') view = await viewApp(r.arg);
    else view = emptyState('Page not found', 'That page does not exist.', 'Go to Home', '#/home');
    if (seq !== state.routeSeq) return;  // user already moved on
    main.replaceChildren(...[].concat(view));
    main.classList.add('enter');
  } catch (e) {
    if (seq !== state.routeSeq || e.status === 401) return;
    main.replaceChildren(errorState(e));
  }
  main.removeAttribute('aria-busy');
  afterRender(r);
}

function afterRender(r) {
  const main = $('#main');
  window.scrollTo(0, 0);
  if (document.body.dataset.input === 'remote') {
    const first = main.querySelector('a[href], button:not([disabled]), input, select');
    (first || main).focus({ preventScroll: true });
  } else if (r.name === 'search' && document.body.dataset.input === 'pointer') {
    const box = $('#q'); if (box) box.focus();
  } else {
    const t = main.querySelector('h1');
    if (t) { t.setAttribute('tabindex', '-1'); t.focus({ preventScroll: true }); }
  }
}

window.addEventListener('hashchange', route);

function emptyState(title, text, actionLabel, href) {
  return h('div', { class: 'empty' }, h('h2', { text: title }), h('p', { text }),
    actionLabel ? h('p', null, h('a', { class: 'btn primary', href, text: actionLabel })) : null);
}

function errorState(e) {
  const retry = h('button', { class: 'btn primary', type: 'button', onclick: async () => {
    try { await api('/api/refresh', { method: 'POST', body: {} }); } catch (_) { /* shown by route */ }
    route();
  }, text: 'Try again' });
  return h('div', { class: 'empty' }, h('h2', { text: "The store couldn't load this" }), h('p', { text: e.message }), h('p', null, retry));
}

// ---------------------------------------------------------------- shared components
function stateChips(app) {
  const chips = [];
  const st = app.state, c = app.compat;
  if (st.action === 'update') chips.push(h('span', { class: 'chip blue', text: 'Update available' }));
  else if (st.action === 'installed' || st.action === 'open') chips.push(h('span', { class: 'chip green', text: 'Installed' }));
  if (!c.ok) chips.push(h('span', { class: 'chip', text: 'Not for this device' }));
  else {
    if (c.universal) chips.push(h('span', { class: 'chip', text: 'Works on all devices' }));
    if (c.kind && c.kind !== 'native') chips.push(h('span', { class: 'chip', text: c.kindLabel }));
  }
  return chips.length ? h('div', { class: 'chips' }, chips) : null;
}

function appCard(app) {
  return h('a', { class: 'card', href: '#/app/' + encodeURIComponent(app.appId), 'aria-label': app.name + ', ' + app.publisher.name },
    avatar(app),
    h('div', { class: 'body' },
      h('div', { class: 'name', text: app.name }),
      h('div', { class: 'pub', text: app.publisher.name }),
      h('div', { class: 'summary', text: app.summary }),
      stateChips(app)));
}

function ratingText(app) {
  return app.rating ? app.rating.average.toFixed(1) + ' out of 5 (' + app.rating.count + ' ratings)' : '';
}

function sectionHead(title, extra) {
  return h('div', { class: 'row-head' }, h('h2', { text: title }), extra || null);
}

function appGrid(apps) { return h('div', { class: 'grid' }, apps.map(appCard)); }

function updateAllButton(n) {
  return h('button', { class: 'btn primary', type: 'button', onclick: async (ev) => {
    ev.currentTarget.disabled = true;
    try { await api('/api/update-all', { method: 'POST', body: {} }); startPolling(); }
    catch (e) { toast(e.message); }
  }, text: n === 1 ? 'Update 1 app' : 'Update all (' + n + ')' });
}

function updatesSection(updates) {
  if (!updates.length) return null;
  return h('section', { class: 'section' },
    sectionHead('Updates', updateAllButton(updates.length)),
    h('div', { class: 'grid' }, updates.map((u) => {
      const a = appCard(u);
      a.querySelector('.pub').textContent = u.publisher.name + ' · ' + u.from + ' → ' + u.to;
      return a;
    })));
}

// ---------------------------------------------------------------- loading placeholders
function skelCard() {
  return h('div', { class: 'skel-card', 'aria-hidden': 'true' }, h('div', { class: 'skel square' }),
    h('div', { class: 'body' }, h('div', { class: 'skel line w60' }), h('div', { class: 'skel line w40' }), h('div', { class: 'skel line w90' })));
}
function skeleton(kind) {
  const label = h('span', { class: 'sr', text: 'Loading' });
  if (kind === 'app')
    return [label, h('div', { class: 'hero', 'aria-hidden': 'true' }, h('div', { class: 'skel square big' }),
      h('div', { class: 'meta' }, h('div', { class: 'skel title' }), h('div', { class: 'skel line w40' }), h('div', { class: 'skel line w90' }))),
      h('div', { class: 'skel pill', 'aria-hidden': 'true' }),
      h('div', { class: 'panel', 'aria-hidden': 'true' }, h('div', { class: 'skel line w40' }), h('div', { class: 'skel line w90' }), h('div', { class: 'skel line w90' }), h('div', { class: 'skel line w60' }))];
  return [label, h('div', { class: 'page-head', 'aria-hidden': 'true' }, h('div', { class: 'skel title' })),
    h('div', { class: 'grid' }, Array.from({ length: 6 }, skelCard))];
}

// ---------------------------------------------------------------- views
async function viewHome() {
  const d = await api('/api/home');
  const out = [h('div', { class: 'page-head' }, h('div', null, h('h1', { text: 'Home' }),
    h('p', { class: 'sub device-note', text: 'Showing apps for ' + state.session.device.describe })))];
  const up = updatesSection(d.updates);
  if (up) out.push(up);
  if (d.featured.length)
    out.push(h('section', { class: 'section' }, sectionHead('Featured'), h('div', { class: 'hscroll' }, d.featured.map(appCard))));
  out.push(h('section', { class: 'section' }, sectionHead('Recently updated'), appGrid(d.recent)));
  out.push(h('section', { class: 'section' }, sectionHead('Browse by category', h('a', { href: '#/explore', text: 'See all' })), categoryTiles(d.categories.slice(0, 6))));
  return out;
}

function categoryTiles(cats) {
  return h('div', { class: 'tiles' }, cats.map((c) =>
    h('a', { class: 'tile', href: '#/category/' + encodeURIComponent(c.id) },
      h('span', { class: 'tname', text: c.name }),
      h('span', { class: 'count', text: c.count === 1 ? '1 app' : c.count + ' apps' }))));
}

async function viewExplore() {
  const d = await api('/api/home');
  return [h('div', { class: 'page-head' }, h('h1', { text: 'Explore' })), categoryTiles(d.categories)];
}

function compatToggle(checked, onchange) {
  return h('label', { class: 'check' }, h('input', { type: 'checkbox', checked: checked ? true : null, onchange: (e) => onchange(e.target.checked) }),
    h('span', { text: 'Only apps that work on this device' }));
}

async function viewCategory(id) {
  const home = await api('/api/home');
  const cat = home.categories.find((c) => c.id === id);
  if (!cat) return emptyState('Category not found', 'There is no such category.', 'Explore categories', '#/explore');
  const list = h('div');
  let only = false;
  const load = async () => {
    const r = await api('/api/apps?category=' + encodeURIComponent(id) + '&sort=name' + (only ? '&compatible=1' : ''));
    list.replaceChildren(r.apps.length ? appGrid(r.apps) : emptyState('Nothing here', only ? 'No apps in this category work on this device.' : 'No apps in this category yet.'));
  };
  await load();
  return [h('div', { class: 'page-head' }, h('div', null, h('h1', { text: cat.name }), h('p', { class: 'sub', text: cat.count + (cat.count === 1 ? ' app' : ' apps') })),
    compatToggle(false, (v) => { only = v; load(); })), list];
}

async function viewSearch(initial) {
  const results = h('div');
  const input = h('input', { id: 'q', type: 'search', placeholder: 'Search apps', 'aria-label': 'Search apps', autocomplete: 'off', enterkeyhint: 'search', value: initial });
  const sort = h('select', { 'aria-label': 'Sort results' },
    h('option', { value: 'relevance', text: 'Best match' }), h('option', { value: 'updated', text: 'Recently updated' }), h('option', { value: 'name', text: 'A to Z' }));
  let only = false, timer = null, seq = 0;
  const run = async () => {
    const my = ++seq;
    const q = input.value.trim();
    history.replaceState(null, '', '#/search' + (q ? '?q=' + encodeURIComponent(q) : ''));
    if (!q) { results.replaceChildren(h('p', { class: 'muted', text: 'Type a name, a topic or a publisher.' })); return; }
    try {
      const r = await api('/api/apps?q=' + encodeURIComponent(q) + '&sort=' + sort.value + (only ? '&compatible=1' : ''));
      if (my !== seq) return;
      results.replaceChildren(r.apps.length
        ? h('div', null, h('p', { class: 'sub', 'aria-live': 'polite', text: r.total + (r.total === 1 ? ' result' : ' results') }), h('div', { class: 'section' }, appGrid(r.apps)))
        : emptyState('No apps found', 'Try fewer or different words.'));
    } catch (e) { if (my === seq) results.replaceChildren(errorState(e)); }
  };
  input.addEventListener('input', () => { clearTimeout(timer); timer = setTimeout(run, 200); });
  sort.addEventListener('change', run);
  await run();
  return [h('div', { class: 'page-head' }, h('h1', { text: 'Search' })),
    h('div', { class: 'search-box' }, input, sort, compatToggle(false, (v) => { only = v; run(); })), h('div', { class: 'section' }, results)];
}

async function viewLibrary() {
  const [d] = await Promise.all([api('/api/library')]);
  const out = [h('div', { class: 'page-head' }, h('h1', { text: 'Library' }))];
  const up = updatesSection(d.updates);
  if (up) out.push(up);
  out.push(h('section', { class: 'section' }, sectionHead('Installed'),
    d.installed.length
      ? h('div', { class: 'grid' }, d.installed.map((i) => {
          const a = i.compat ? appCard(i) : h('a', { class: 'card', href: '#/app/' + encodeURIComponent(i.appId) },
            avatar(i), h('div', { class: 'body' }, h('div', { class: 'name', text: i.name }), h('div', { class: 'summary', text: i.summary })));
          const pub = a.querySelector('.pub');
          if (pub) pub.textContent = (i.publisher ? i.publisher.name + ' · ' : '') + 'Version ' + i.installedVersion;
          return a;
        }))
      : emptyState('Nothing installed yet', 'Apps you install appear here.', 'Find apps', '#/explore')));
  if (d.history.length)
    out.push(h('section', { class: 'section' }, sectionHead('Recent activity'),
      h('ul', { class: 'hist' }, d.history.map((e) => h('li', null,
        h('span', { text: cap(e.action) + ' ' + e.appId + (e.version ? ' ' + e.version : '') + ' — ' + e.outcome }),
        h('span', { class: 'muted', text: fmtDate(e.time) }))))));
  out.push(h('section', { class: 'section' }, sectionHead('About this store'), aboutPanel()));
  return out;
}

function cap(s) { return s ? s[0].toUpperCase() + s.slice(1) : s; }

function aboutPanel() {
  const s = state.session;
  return h('div', { class: 'trust' },
    h('dl', { class: 'facts' },
      h('dt', { text: 'This device' }), h('dd', { text: s.device.describe }),
      h('dt', { text: 'Catalog' }), h('dd', { text: s.catalog.available ? s.catalog.apps + ' apps' + (s.catalog.fromCache ? ' (saved copy)' : '') + (s.catalog.skipped ? ', ' + s.catalog.skipped + ' skipped as invalid' : '') : 'not loaded' }),
      h('dt', { text: 'Installer' }), h('dd', { text: s.store.backend }),
      h('dt', { text: 'Signature check' }), h('dd', { text: s.store.verifier + (s.store.verifierAvailable ? '' : ' — installs are disabled') + ', ' + s.store.trustedKeys + ' trusted publisher keys' })),
    h('p', { class: 'note', text: s.store.posture }));
}

async function viewApp(id) {
  const a = await api('/api/apps/' + encodeURIComponent(id));
  const active = state.tasks.find((t) => !t.finished && t.appId === id);
  const st = a.state;
  const block = h('div', { class: 'action-block' });
  if (active) block.append(progressCard(active));
  else if (st.action === 'install' || st.action === 'update' || st.action === 'open') {
    block.append(h('button', { class: 'btn primary', type: 'button', onclick: (ev) => primaryAction(ev.currentTarget, a), text: st.label }));
  } else if (st.action === 'installed') {
    block.append(h('span', { class: 'status-pill ok' }, checkMark(), 'Installed' + (st.installedVersion ? ' · ' + st.installedVersion : '')));
  } else {
    block.append(h('span', { class: 'status-pill no', text: st.label || 'Not available for this device' }));
  }
  if (st.note) block.append(h('p', { class: 'note', text: st.note }));
  if (!a.compat.ok && a.compat.reason) block.append(h('p', { class: 'note', text: a.compat.reason }));
  if (st.installedVersion && !active)
    block.append(h('button', { class: 'btn link', type: 'button', onclick: () => confirmUninstall(a), text: 'Uninstall' }));

  const facts = h('dl', { class: 'facts' },
    h('dt', { text: 'Devices' }), h('dd', { text: a.compat.devices.join(', ') }),
    h('dt', { text: 'Processors' }), h('dd', { text: a.architectures.join(', ') }),
    h('dt', { text: 'Exile OS' }), h('dd', { text: a.minimumOs ? a.minimumOs + ' or newer' : 'Any version' }),
    a.compat.kindLabel ? [h('dt', { text: 'Type on this device' }), h('dd', { text: a.compat.kindLabel })] : null,
    a.downloadSize ? [h('dt', { text: 'Download' }), h('dd', { text: humanSize(a.downloadSize) })] : null,
    a.installedSize ? [h('dt', { text: 'Installed size' }), h('dd', { text: humanSize(a.installedSize) })] : null,
    h('dt', { text: 'Version' }), h('dd', { text: a.version }),
    a.updatedAt ? [h('dt', { text: 'Updated' }), h('dd', { text: fmtDate(a.updatedAt) })] : null,
    a.license ? [h('dt', { text: 'License' }), h('dd', { text: a.license })] : null,
    a.privacyPolicy ? [h('dt', { text: 'Privacy policy' }), h('dd', { text: a.privacyPolicy })] : null,
    a.sourceRepository ? [h('dt', { text: 'Source code' }), h('dd', { text: a.sourceRepository })] : null);

  const perms = a.permissions.length
    ? h('ul', { class: 'list' }, a.permissions.map((p) => h('li', null, p.text, p.reason ? h('div', { class: 'sub', text: 'Publisher says: ' + p.reason }) : null)))
    : h('p', { text: 'This app asks for no special permissions.' });

  let trust = null;
  if (a.trust) {
    trust = h('div', { class: 'trust ' + (a.trust.trusted ? 'good' : 'bad') },
      h('p', { text: a.trust.trusted
        ? "Signature valid, made with trusted key '" + a.trust.keyId + "' for this publisher. The package is checked again before it is installed."
        : 'Not verified: ' + a.trust.reason + '. Installation is blocked.' }),
      h('p', { class: 'note', text: a.trust.caveat }));
  }
  const needs = a.dependencies && a.dependencies.length
    ? h('section', { class: 'panel' }, h('h2', { text: 'Needs' }),
        h('ul', { class: 'list' }, a.dependencies.map((d) => h('li', null,
          h('a', { href: '#/app/' + encodeURIComponent(d.appId), text: d.name }),
          (d.minVersion ? ' ' + d.minVersion + ' or newer' : '') + (d.installed ? ' (installed)' : ' (not installed)')))))
    : null;

  const left = [
    a.description ? h('section', { class: 'panel' }, h('h2', { text: 'About' }), h('p', { text: a.description })) : null,
    a.releaseNotes ? h('section', { class: 'panel' }, h('h2', { text: "What's new in " + a.version }), h('p', { text: a.releaseNotes })) : null,
    h('section', { class: 'panel' }, h('h2', { text: 'Permissions' }), perms),
    needs,
  ];
  const right = [
    h('section', { class: 'panel' }, h('h2', { text: 'Details' }), facts),
    h('section', { class: 'panel' }, h('h2', { text: 'Publisher' }), h('p', { text: a.publisher.name + ' (' + a.publisher.id + ')' }), trust),
  ];
  const rt = ratingText(a);
  return [
    h('div', { class: 'page-head' }, h('button', { class: 'btn link', type: 'button', onclick: goBack, text: '← Back' })),
    h('div', { class: 'hero' }, avatar(a, true), h('div', { class: 'meta' },
      h('h1', { text: a.name }),
      h('p', { class: 'sub', text: a.publisher.name + (rt ? ' · ' + rt : '') }),
      h('p', { text: a.summary }),
      stateChips(a))),
    block,
    h('div', { class: 'two-col' }, h('div', null, left), h('div', null, right)),
  ];
}

async function primaryAction(btn, app) {
  const st = app.state;
  if (st.action === 'open') { toast('Opening apps is not available on this system yet.'); return; }
  btn.disabled = true;
  try {
    await api(st.action === 'update' ? '/api/update' : '/api/install', { method: 'POST', body: { appId: app.appId } });
    await startPolling();
    route();
  } catch (e) { btn.disabled = false; toast(e.message); }
}

async function confirmUninstall(app) {
  const ok = await confirmDialog('Uninstall ' + app.name + '?', 'The app and its files on this device are removed.', 'Uninstall', true);
  if (!ok) return;
  try { await api('/api/uninstall', { method: 'POST', body: { appId: app.appId } }); await startPolling(); route(); }
  catch (e) { toast(e.message); }
}

// ---------------------------------------------------------------- tasks / activity tray
const STEPS = [['Check', ['Queued', 'Checking compatibility', 'Checking requirements', 'Checking storage']],
               ['Download', ['Downloading']], ['Verify', ['Verifying']], ['Install', ['Installing']]];
function stepper(t) {
  if (t.kind === 'uninstall') return null;
  const idx = t.finished ? STEPS.length : Math.max(0, STEPS.findIndex(([, states]) => states.includes(t.state)));
  const tick = () => { const n = document.createElementNS('http://www.w3.org/2000/svg', 'svg'); n.setAttribute('viewBox', '0 0 12 12'); n.setAttribute('aria-hidden', 'true');
    const p = document.createElementNS('http://www.w3.org/2000/svg', 'path'); p.setAttribute('d', 'M2 6.5l2.5 2.5L10 3.5'); p.setAttribute('fill', 'none'); p.setAttribute('stroke', 'currentColor'); p.setAttribute('stroke-width', '2'); p.setAttribute('stroke-linecap', 'round'); p.setAttribute('stroke-linejoin', 'round'); n.append(p); return n; };
  return h('ol', { class: 'steps', 'aria-label': 'Progress' }, STEPS.map(([label], i) => {
    const cls = i < idx ? 'done' : i === idx ? 'now' : '';
    return h('li', { class: cls, 'aria-current': i === idx ? 'step' : null },
      h('span', { class: 'dot' }, i < idx ? tick() : null), h('span', { class: 'label', text: label }), i < STEPS.length - 1 ? h('span', { class: 'join' }) : null);
  }));
}

function progressCard(t) {
  const pct = t.total > 0 ? Math.min(100, Math.round((t.done / t.total) * 100)) : 0;
  const bar = h('div', { class: 'bar' + (t.total > 0 ? '' : ' indeterminate'), role: 'progressbar', 'aria-valuemin': '0', 'aria-valuemax': '100', 'aria-valuenow': t.total > 0 ? String(pct) : null }, h('i'));
  if (t.total > 0) bar.firstChild.style.width = pct + '%';
  return h('div', { class: 'progress-card' }, h('div', { text: verbs(t)[0] + '… ' + t.message }), stepper(t), bar,
    t.cancellable ? h('p', null, h('button', { class: 'btn link', type: 'button', onclick: () => cancelTask(t.id), text: 'Cancel' })) : null);
}

function verbs(t) {
  return { install: ['Installing', 'Installed', "Couldn't install"], update: ['Updating', 'Updated', "Couldn't update"], uninstall: ['Removing', 'Removed', "Couldn't remove"] }[t.kind];
}

async function cancelTask(id) {
  try { await api('/api/tasks/' + id + '/cancel', { method: 'POST', body: {} }); startPolling(); }
  catch (e) { toast(e.message); }
}

function toastNode(t) {
  const [doing, done, failed] = verbs(t);
  const cancelled = t.finished && t.state === 'Cancelled';
  const cls = !t.finished ? '' : t.ok ? ' done' : ' fail';
  const title = !t.finished ? doing + ' ' + t.appName : t.ok ? done + ' ' + t.appName : cancelled ? 'Cancelled: ' + t.appName : failed + ' ' + t.appName;
  const node = h('div', { class: 'toast' + cls, role: t.finished && !t.ok ? 'alert' : null });
  const row = h('div', { class: 't-row' }, h('div', { class: 't-title', text: title }));
  if (t.finished) row.append(h('button', { class: 'btn', type: 'button', onclick: () => { state.dismissed.add(t.id); renderTray(); }, text: 'Dismiss' }));
  else if (t.cancellable) row.append(h('button', { class: 'btn', type: 'button', onclick: () => cancelTask(t.id), text: 'Cancel' }));
  node.append(row);
  const msg = t.finished ? (t.ok ? t.detail : t.error) : t.message;
  if (msg) node.append(h('div', { class: 't-msg', text: msg }));
  if (!t.finished) {
    const st = stepper(t); if (st) node.append(st);
    const pct = t.total > 0 ? Math.min(100, Math.round((t.done / t.total) * 100)) : 0;
    const bar = h('div', { class: 'bar' + (t.total > 0 ? '' : ' indeterminate') }, h('i'));
    if (t.total > 0) bar.firstChild.style.width = pct + '%';
    node.append(bar);
  }
  return node;
}

function renderTray() {
  const tray = $('#tray');
  const visible = state.tasks.filter((t) => !state.dismissed.has(t.id) && (!t.finished || state.shown.has(t.id)));
  tray.replaceChildren(...visible.slice(0, 4).map(toastNode));
}
state.shown = new Set();

function toast(message) {
  const id = 'm' + Date.now();
  const node = h('div', { class: 'toast', role: 'alert' }, h('div', { class: 't-msg', text: message }));
  $('#tray').append(node);
  setTimeout(() => node.remove(), 6000);
  return id;
}

async function pollOnce() {
  const r = await api('/api/tasks');
  const finishedNow = [];
  for (const t of r.tasks) {
    const was = state.seen.get(t.id);
    // A task can start and finish between two polls, so a finished task we never saw
    // running still counts as news (except tasks that predate this page load).
    if (t.finished && (was === false || (was === undefined && state.booted))) { finishedNow.push(t); state.shown.add(t.id); }
    state.seen.set(t.id, t.finished);
    if (!t.finished) state.shown.add(t.id);
  }
  state.tasks = r.tasks;
  for (const t of finishedNow) {
    if (t.ok) setTimeout(() => { state.dismissed.add(t.id); renderTray(); }, 5000);
  }
  renderTray();
  if (finishedNow.length) {
    try { state.session = await api('/api/session'); } catch (_) { /* keep old */ }
    refreshInPlace();
  }
  return r.tasks.some((t) => !t.finished);
}

// Re-render the current screen without stealing focus or scroll position.
async function refreshInPlace() {
  const r = parseRoute();
  if (!['home', 'library', 'app', 'category'].includes(r.name)) return;
  const y = window.scrollY;
  const focusedHref = document.activeElement && document.activeElement.getAttribute && document.activeElement.getAttribute('href');
  const seq = ++state.routeSeq;
  try {
    const view = r.name === 'home' ? await viewHome() : r.name === 'library' ? await viewLibrary()
               : r.name === 'category' ? await viewCategory(r.arg) : await viewApp(r.arg);
    if (seq !== state.routeSeq) return;
    $('#main').classList.remove('enter');   // a background refresh must not replay the entrance
    $('#main').replaceChildren(...[].concat(view));
    window.scrollTo(0, y);
    if (focusedHref) { const el = $('#main a[href="' + CSS.escape(focusedHref) + '"]'); if (el) el.focus({ preventScroll: true }); }
  } catch (_) { /* leave the old view */ }
}

async function startPolling() {
  if (state.poll) return;
  state.poll = true;
  try {
    for (let busy = true; busy;) {
      busy = await pollOnce();
      if (busy) await new Promise((res) => setTimeout(res, 400));
    }
  } catch (_) { /* api() already reported auth problems */ }
  state.poll = null;
}

// ---------------------------------------------------------------- modal
function confirmDialog(title, body, confirmLabel, danger) {
  return new Promise((resolve) => {
    const root = $('#modal-root');
    const previous = document.activeElement;
    const close = (v) => { root.replaceChildren(); document.removeEventListener('keydown', onKey, true); if (previous && previous.focus) previous.focus(); resolve(v); };
    const onKey = (e) => { if (e.key === 'Escape' || isBackKey(e)) { e.preventDefault(); e.stopPropagation(); close(false); } };
    const cancel = h('button', { class: 'btn', type: 'button', onclick: () => close(false), text: 'Cancel' });
    const ok = h('button', { class: 'btn ' + (danger ? 'danger' : 'primary'), type: 'button', onclick: () => close(true), text: confirmLabel });
    root.replaceChildren(h('div', { class: 'scrim' }, h('div', { class: 'dialog', role: 'dialog', 'aria-modal': 'true', 'aria-labelledby': 'dlg-title' },
      h('h2', { id: 'dlg-title', text: title }), h('p', { text: body }), h('div', { class: 'actions' }, cancel, ok))));
    document.addEventListener('keydown', onKey, true);
    cancel.focus();
  });
}

// ---------------------------------------------------------------- keyboard, remote control
const BACK_CODES = new Set([10009, 461, 4, 27]);       // Tizen, webOS, Android, Escape
function isBackKey(e) {
  if (e.key === 'BrowserBack' || e.key === 'GoBack' || e.key === 'XF86Back' || BACK_CODES.has(e.keyCode)) return true;
  const t = e.target;
  const typing = t && (t.tagName === 'INPUT' || t.tagName === 'TEXTAREA' || t.tagName === 'SELECT' || t.isContentEditable);
  return e.key === 'Backspace' && !typing;
}

function visible(el) {
  const r = el.getBoundingClientRect();
  if (r.width === 0 || r.height === 0) return false;
  const cs = getComputedStyle(el);
  return cs.visibility !== 'hidden' && cs.display !== 'none';
}

function focusables() {
  const modal = $('#modal-root .dialog');
  const scope = modal || document;
  return [...scope.querySelectorAll('a[href]:not(.skip), button:not([disabled]), input:not([disabled]), select:not([disabled]), [tabindex="0"]')].filter(visible);
}

// Directional focus: pick the closest focusable that lies in the pressed direction.
function moveFocus(dx, dy) {
  const all = focusables();
  if (!all.length) return false;
  const cur = all.includes(document.activeElement) ? document.activeElement : null;
  if (!cur) { all[0].focus(); return true; }
  const a = cur.getBoundingClientRect();
  const ax = a.left + a.width / 2, ay = a.top + a.height / 2;
  let best = null, bestScore = Infinity;
  for (const el of all) {
    if (el === cur) continue;
    const b = el.getBoundingClientRect();
    const bx = b.left + b.width / 2, by = b.top + b.height / 2;
    // The candidate must lie entirely beyond the current element in the pressed direction.
    if (dx > 0 && b.left < a.right - 2) continue;
    if (dx < 0 && b.right > a.left + 2) continue;
    if (dy > 0 && b.top < a.bottom - 2) continue;
    if (dy < 0 && b.bottom > a.top + 2) continue;
    const along = dx ? (bx - ax) * dx : (by - ay) * dy;
    const across = dx ? Math.abs(by - ay) : Math.abs(bx - ax);
    const overlap = dx ? !(b.bottom < a.top || b.top > a.bottom) : !(b.right < a.left || b.left > a.right);
    const score = along + across * (overlap ? 0.5 : 3);
    if (score < bestScore) { bestScore = score; best = el; }
  }
  if (best) { best.focus({ preventScroll: true }); best.scrollIntoView({ block: 'nearest', inline: 'nearest' }); return true; }
  return false;
}

document.addEventListener('keydown', (e) => {
  if (e.defaultPrevented || e.altKey || e.ctrlKey || e.metaKey) return;
  const remote = document.body.dataset.input === 'remote';
  const modalOpen = !!$('#modal-root .dialog');
  if (modalOpen) return handleModalTab(e);
  if (remote) {
    const dirs = { ArrowLeft: [-1, 0], ArrowRight: [1, 0], ArrowUp: [0, -1], ArrowDown: [0, 1] };
    const d = dirs[e.key];
    const t = e.target;
    const textual = t && (t.tagName === 'INPUT' && t.type !== 'checkbox' || t.tagName === 'SELECT');
    if (d && !(textual && d[0] !== 0) && !(t && t.tagName === 'SELECT')) {
      if (moveFocus(d[0], d[1])) e.preventDefault();
      return;
    }
    if (e.key === 'Enter' && t && t.getAttribute && t.getAttribute('role') === 'button') { t.click(); e.preventDefault(); return; }
  }
  if (isBackKey(e)) {
    const r = parseRoute();
    if (remote) { if (r.name !== 'home') { e.preventDefault(); goBack(); } else if (e.key === 'Backspace') e.preventDefault(); }
    else if (e.key === 'Escape' && r.name === 'app') { e.preventDefault(); goBack(); }
    return;
  }
  if (e.key === '/' && !remote && !(e.target && e.target.tagName === 'INPUT')) { e.preventDefault(); location.hash = '#/search'; }
});

function handleModalTab(e) {
  if (e.key !== 'Tab') return;
  const f = focusables();
  if (!f.length) return;
  const first = f[0], last = f[f.length - 1];
  if (e.shiftKey && document.activeElement === first) { e.preventDefault(); last.focus(); }
  else if (!e.shiftKey && document.activeElement === last) { e.preventDefault(); first.focus(); }
}

// ---------------------------------------------------------------- start up
// The splash covers only the first load of a tab. It is never held back artificially
// when the user prefers reduced motion, and it never blocks error screens.
const splashStart = performance.now();
function hideSplash() {
  const el = $('#splash');
  if (!el || el.classList.contains('gone')) return;
  const first = safeStorage('get', 'exile-splash') !== '1';
  safeStorage('set', 'exile-splash', '1');
  const reduce = matchMedia('(prefers-reduced-motion: reduce)').matches;
  const minimum = first && !reduce ? 900 : 0;       // lets the mark finish drawing once per tab
  const wait = Math.max(0, minimum - (performance.now() - splashStart));
  setTimeout(() => { el.classList.add('leaving'); setTimeout(() => el.classList.add('gone'), 320); }, wait);
}

function showNoAccess() {
  hideSplash();
  state.token = null;
  safeStorage('remove', 'exile-token');
  $('#nav').replaceChildren();
  $('#banners').replaceChildren();
  $('#main').replaceChildren(h('div', { class: 'empty' }, h('h2', { text: 'Open the store from its link' }),
    h('p', { text: 'This page needs the access link that the store printed when it started. Open that link again on this device.' })));
}

function renderBanners() {
  const b = $('#banners');
  const s = state.session;
  const items = [];
  if (!s.catalog.available) items.push(h('div', { class: 'banner warn' }, h('strong', { text: 'Catalog unavailable. ' }), s.catalog.notice || 'No apps can be shown.'));
  else if (s.catalog.fromCache || s.catalog.notice) items.push(h('div', { class: 'banner warn' }, h('strong', { text: 'Offline. ' }), s.catalog.notice || 'Showing the last saved catalog.'));
  for (const n of s.notices) items.push(h('div', { class: 'banner warn' }, n));
  b.replaceChildren(...items);
}

async function boot() {
  const frag = new URLSearchParams(location.hash.replace(/^#/, ''));
  const t = frag.get('t');
  if (t) {
    safeStorage('set', 'exile-token', t);
    if (frag.get('ui')) safeStorage('set', 'exile-ui', frag.get('ui'));
    history.replaceState(null, '', location.pathname + '#/home');   // the key never stays in the address bar
  }
  state.token = safeStorage('get', 'exile-token');
  state.forcedUi = safeStorage('get', 'exile-ui');
  if (!state.token) { showNoAccess(); return; }
  try {
    state.session = await api('/api/session');
  } catch (e) {
    if (e.status !== 401) { $('#main').replaceChildren(errorState(e)); hideSplash(); }
    return;
  }
  computeUi();
  renderBanners();
  await pollOnce().then((busy) => { if (busy) startPolling(); }).catch(() => {});
  state.booted = true;
  route();         // draws the skeleton synchronously, then fills in
  hideSplash();    // reveal it: slow data shows placeholders, not a stuck splash
}

boot();
})();
