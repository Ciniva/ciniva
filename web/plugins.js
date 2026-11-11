import { api, img } from './api.js';
import { h, icon, toast, modal } from './dom.js';
import { openLive } from './live.js';

let cache = null;

export async function plugins(force) {
  if (!cache || force) cache = api('/plugins').catch(() => []);
  return cache;
}

export function actionsFor(list, kind) {
  return list.filter((p) => p.online).flatMap((p) => (p.actions || []).filter((a) => (a.contexts || []).includes(kind)).map((a) => ({ plugin: p, action: a })));
}

function call(pluginId, c, extra) {
  const body = { ...(c.body || {}), ...(extra || {}) };
  const method = (c.method || 'POST').toUpperCase();
  const path = `/plugins/${pluginId}/ui/${c.path.replace(/^\//, '')}`;
  return method === 'GET' ? api(path) : api(path, { method, body });
}

/// Renders a plugin page at #/plugin/{id}/{page}?params.
export async function pluginView(el, pluginId, page, params) {
  const host = h('div.plugin-host');
  el.append(host);
  await renderPage(host, pluginId, page, params);
}

async function renderPage(host, pluginId, page, params) {
  const qs = params.toString();
  const load = () => api(`/plugins/${pluginId}/ui/pages/${page}${qs ? '?' + qs : ''}`);
  const ctx = { pluginId, page, params, host, reload: null };
  let timer = null;
  const draw = (view) => {
    clearTimeout(timer);
    host.replaceChildren(renderView(view, ctx));
    if (view.refresh && document.body.contains(host)) {
      timer = setTimeout(async () => {
        if (!document.body.contains(host) || host.querySelector(':focus')) {
          if (document.body.contains(host)) draw(view);
          return;
        }
        try { draw(await load()); } catch {}
      }, view.refresh * 1000);
    }
  };
  ctx.reload = async () => draw(await load());
  ctx.show = draw;
  draw(await load());
}

export async function runAction(plugin, action, context) {
  try {
    const result = await api(`/plugins/${plugin.id}/ui/actions/${action.id}`, { method: 'POST', body: { context } });
    handleResult(result, { pluginId: plugin.id, inModal: true });
  } catch (e) {
    toast(e.message, 'error');
  }
}

function handleResult(result, ctx) {
  if (!result) return;
  if (result.message) toast(result.message, result.tone === 'error' ? 'error' : 'info');
  if (result.navigate) {
    navigate(ctx.pluginId, result.navigate);
  } else if (result.view) {
    if (ctx.show && !ctx.inModal) ctx.show(result.view);
    else {
      const holder = h('div.plugin-modal');
      const sub = { ...ctx, host: holder, show: (v) => holder.replaceChildren(renderView(v, sub)), reload: async () => {} };
      holder.append(renderView(result.view, sub));
      modal(holder, { wide: true });
    }
  } else if (result.refresh && ctx.reload) {
    ctx.reload();
  }
}

function navigate(pluginId, nav) {
  const qs = new URLSearchParams(nav.params || {}).toString();
  document.querySelector('dialog.modal[open]')?.close();
  location.hash = `#/plugin/${pluginId}/${nav.page}${qs ? '?' + qs : ''}`;
}

function play(ctx, p, siblings) {
  openLive({ plugin: ctx.pluginId, stream: p.stream, title: p.title, image: p.image, channels: siblings });
}

async function perform(action, ctx, extraBody) {
  if (action.confirm && !confirm(action.confirm)) return;
  if (action.play) return play(ctx, action.play);
  if (action.navigate) return navigate(ctx.pluginId, action.navigate);
  if (!action.call) return;
  try {
    handleResult(await call(ctx.pluginId, action.call, extraBody), ctx);
  } catch (e) {
    toast(e.message, 'error');
  }
}

function actionButton(a, ctx, small) {
  const cls = `button.btn${small ? '.small' : ''}${a.tone === 'primary' ? '.primary' : a.tone === 'danger' ? '.danger' : ''}`;
  return h(cls, { type: 'button', onclick: (e) => { e.stopPropagation(); perform(a, ctx); } }, a.icon ? icon(a.icon) : null, a.icon ? ' ' : null, a.label);
}

const TONES = new Set(['info', 'success', 'warning', 'error']);

function renderView(view, ctx) {
  return h('div.plugin-view',
    h('div.page-head', h('h1', view.title || ''), view.subtitle ? h('span.muted', view.subtitle) : null),
    (view.blocks || []).map((b) => renderBlock(b, ctx)),
  );
}

function renderBlock(b, ctx) {
  switch (b.type) {
    case 'text':
      return h(`p.plugin-text${TONES.has(b.tone) ? '.' + b.tone : ''}`, b.text);
    case 'stats':
      return h('div.plugin-stats', (b.items || []).map((s) => h('div.stat', h('div.stat-n', String(s.value ?? '')), h('div.stat-l', s.label))));
    case 'actions':
      return h('div.actions.plugin-actions', (b.items || []).map((a) => actionButton(a, ctx)));
    case 'form':
      return renderForm(b, ctx);
    case 'list':
      return renderList(b, ctx);
    default:
      return null;
  }
}

function renderList(b, ctx) {
  const items = b.items || [];
  if (!items.length) return h('div.empty.plugin-empty', h('p', b.empty || 'Nothing here.'));
  const channels = items.filter((it) => it.play).map((it) => ({ stream: it.play.stream, title: it.play.title || it.title, image: it.image }));
  return h('div.plugin-list', items.map((it) => {
    const row = h('div.plugin-item' + (it.link || it.play ? '.clickable' : '') + (it.play ? '.playable' : ''),
      it.play && !it.image ? h('div.plugin-item-img.logo', icon('tv')) : null,
      it.image ? h('img.plugin-item-img' + (it.play ? '.logo' : ''), {
        onerror: (e) => { if (it.play) e.target.replaceWith(h('div.plugin-item-img.logo', icon('tv'))); else e.target.style.visibility = 'hidden'; }, src: it.image.startsWith('/') && !it.image.startsWith('/api/') ? img(it.image, 'w185') : it.image, alt: '', loading: 'lazy' }) : null,
      h('div.plugin-item-body',
        h('div.plugin-item-title', it.title),
        it.subtitle ? h('div.plugin-item-sub', it.subtitle) : null,
        typeof it.progress === 'number' ? h('div.resume-bar', h('div', { style: { width: `${Math.max(0, Math.min(1, it.progress)) * 100}%` } })) : null,
        it.badges?.length ? h('div.badges', it.badges.map((x) => h(`span.tag${TONES.has(x.tone) ? '.' + x.tone : ''}`, x.text))) : null,
      ),
      it.actions?.length ? h('div.plugin-item-actions', it.actions.map((a) => actionButton(a, ctx, true))) : null,
    );
    if (it.play) row.addEventListener('click', () => play(ctx, { title: it.title, image: it.image, ...it.play }, channels));
    else if (it.link) row.addEventListener('click', () => navigate(ctx.pluginId, it.link));
    return row;
  }));
}

function renderForm(b, ctx) {
  const inputs = {};
  const fields = (b.fields || []).map((f) => {
    let input;
    switch (f.kind) {
      case 'select':
        input = h('select', { name: f.name }, (f.options || []).map((o) => h('option', { value: o.value, selected: String(o.value) === String(f.value ?? '') }, o.label)));
        break;
      case 'toggle':
        input = h('input', { name: f.name, type: 'checkbox', checked: !!f.value });
        break;
      case 'textarea':
        input = h('textarea', { name: f.name, rows: 3, placeholder: f.placeholder || '', required: !!f.required }, f.value ?? '');
        break;
      case 'file':
        input = h('input', { name: f.name, type: 'file', required: !!f.required });
        break;
      default:
        input = h('input', { name: f.name, type: f.kind === 'password' ? 'password' : f.kind === 'number' ? 'number' : 'text', value: f.value ?? '', placeholder: f.placeholder || '', required: !!f.required, autocomplete: 'off' });
    }
    inputs[f.name] = { f, input };
    return f.kind === 'toggle'
      ? h('label.check.plugin-field', input, ' ', f.label, f.help ? h('span.muted.small', ` — ${f.help}`) : null)
      : h('label.plugin-field', h('span', f.label), input, f.help ? h('span.muted.small', f.help) : null);
  });
  const submit = h('button.btn.primary', { type: 'submit' }, b.submit?.label || 'Save');
  const form = h('form.plugin-form', {
    onsubmit: async (e) => {
      e.preventDefault();
      submit.disabled = true;
      try {
        const values = {};
        for (const [name, { f, input }] of Object.entries(inputs)) {
          if (f.kind === 'toggle') values[name] = input.checked;
          else if (f.kind === 'number') values[name] = input.value === '' ? null : Number(input.value);
          else if (f.kind === 'file') values[name] = input.files[0] ? await readFile(input.files[0]) : null;
          else values[name] = input.value;
        }
        if (b.submit?.call) await perform({ call: b.submit.call }, ctx, values);
        else if (ctx.onSubmit) await ctx.onSubmit(values);
      } finally {
        submit.disabled = false;
      }
    },
  }, fields, h('div.actions', submit));
  return form;
}

function readFile(file) {
  return new Promise((resolve, reject) => {
    const r = new FileReader();
    r.onload = () => resolve({ name: file.name, data_base64: String(r.result).split(',')[1] || '' });
    r.onerror = () => reject(r.error);
    r.readAsDataURL(file);
  });
}

export async function settingsPanels() {
  const list = await plugins(true);
  const panels = [];
  for (const p of list.filter((x) => x.has_settings)) {
    let fields = [];
    try { fields = (await api(`/plugins/${p.id}/settings`)).fields; } catch { continue; }
    const ctx = {
      pluginId: p.id,
      onSubmit: async (values) => {
        try {
          await api(`/plugins/${p.id}/settings`, { method: 'PUT', body: { values } });
          toast(`${p.name} settings saved`);
        } catch (e) {
          toast(e.message, 'error');
        }
      },
    };
    panels.push(h('section.panel',
      h('div.panel-head', h('h2', p.name), h('span.tag' + (p.online ? '.success' : '.error'), p.online ? 'running' : 'offline'), p.version ? h('span.muted.small', `v${p.version}`) : null),
      p.description ? h('p.muted', p.description) : null,
      renderForm({ fields, submit: { label: 'Save settings' } }, ctx),
    ));
  }
  return panels;
}

export function listenForEvents(onLibrary) {
  if (!window.EventSource) return;
  let source;
  const connect = () => {
    source = new EventSource('/api/events');
    source.addEventListener('notification', (e) => {
      try {
        const n = JSON.parse(e.data);
        toast([n.title, n.body].filter(Boolean).join(' · '), n.tone === 'error' ? 'error' : 'info');
      } catch {}
    });
    source.addEventListener('library', () => onLibrary?.());
    source.onerror = () => {
      source.close();
      setTimeout(connect, 15000);
    };
  };
  connect();
}
