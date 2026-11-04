'use strict';
/* 渲染进程:列表 / 详情 / 进度 */

/* ---- 外壳适配 ----
 * 界面只认 window.api 这一层抽象;Tauri 下用 invoke/event 现场拼出来,
 * 测试里则直接注入一个假的 window.api。
 */
if (!window.api && window.__TAURI__) {
  const { invoke } = window.__TAURI__.core;
  const { listen } = window.__TAURI__.event;
  window.api = {
    pickFiles: () => invoke('pick_files'),
    pickFolder: () => invoke('pick_folder'),
    pickOutDir: () => invoke('pick_output_dir'),
    ensureDir: (p) => invoke('ensure_dir', { path: p }),
    envStatus: () => invoke('env_status'),
    stat: (paths) => invoke('stat_files', { paths }),
    startupPaths: () => invoke('startup_paths'),
    startupFlags: () => invoke('startup_flags'),
    loadSettings: () => invoke('load_settings'),
    saveSettings: (s) => invoke('save_settings', { settings: s }),
    saveQueue: (items) => invoke('save_queue', { items }),
    loadQueue: () => invoke('load_queue'),
    clearQueue: () => invoke('clear_queue'),
    openDir: (p) => invoke('open_dir', { path: p }),
    readAudio: (p) => invoke('read_audio', { path: p }),
    debugReport: (m) => invoke('debug_report', { msg: m }),
    scanPaths: (paths) => invoke('scan_paths', { paths }),
    start: async (payload) => {
      try {
        await invoke('start_batch', { args: payload });
        return { ok: true };
      } catch (e) {
        return { ok: false, error: String(e) };
      }
    },
    cancel: () => invoke('cancel_batch'),
    reveal: (p) => invoke('reveal', { path: p }),
    openPath: (p) => invoke('reveal', { path: p }),
    minimize: () => invoke('win_minimize'),
    maximize: () => invoke('win_maximize'),
    close: () => invoke('win_close'),
    pathForFile: (f) => (f && f.path) || '',
    onDrop: (cb) => { listen('tauri://drag-drop', (e) => cb((e.payload && e.payload.paths) || [])); },
    onStartOne: (cb) => { listen('task-start', (e) => cb(e.payload)); },
    onResult: (cb) => {
      listen('task-result', (e) => {
        const r = e.payload;
        cb({
          srcPath: r.src,
          status: r.status,
          outPath: r.out,
          format: r.format,
          meta: {
            musicName: r.title,
            artist: r.artist ? [[r.artist, 0]] : [],
            album: r.album,
          },
          error: r.errorCode ? { code: r.errorCode, message: r.errorMsg } : null,
          ms: r.ms,
          lrcCopied: r.lrcCopied,
          hasLrc: r.hasLrc,
        });
      });
    },
    onDone: (cb) => { listen('batch-done', (e) => cb(e.payload)); },
  };
}

const $ = (s) => document.querySelector(s);
const rowsEl = $('#rows');
const emptyEl = $('#empty');

const S = {
  items: new Map(),      // path -> item
  order: [],             // path[]
  filter: 'all',
  q: '',
  running: false,
  outDir: null,
  copyLrc: true,
  currentId: null,
  template: '{artist} - {title}',
  overwrite: 'rename',
  concurrency: 0,
  openAfter: false,
};

const DEFAULT_SETTINGS = {
  template: '{artist} - {title}',
  overwrite: 'rename',
  concurrency: 0,
  outDir: null,
  openAfter: false,
  copyLrc: true,
};
let settings = Object.assign({}, DEFAULT_SETTINGS);
let draft = null;
let lastOutDir = null;

/* ---------------- 渲染 ---------------- */
function fmtSize(n) {
  if (!n) return '—';
  return (n / 1048576).toFixed(1) + ' MB';
}
function errLabel(err) {
  if (!err) return '失败';
  const map = {
    E_NO_KEY: '需密钥', E_FORMAT: '格式不支持', E_CORRUPT: '文件损坏',
    E_KEY: '密钥错误', E_META: '元信息损坏', E_FLAC: 'FLAC 结构异常',
    E_EKEY: '需联网取密钥', E_IO: '读写失败',
    E_CANCEL: '已取消', E_UNKNOWN: '失败',
  };
  return map[err.code] || '失败';
}
function statusHTML(it) {
  switch (it.status) {
    case 'running': return '<span class="st run"><i class="dot"></i>转换中</span>';
    case 'done': return '<span class="st done"><i class="dot"></i>完成</span>';
    case 'failed': return `<span class="st err"><i class="dot"></i>${errLabel(it.err)}</span>`;
    case 'skipped': return '<span class="st skip"><i class="dot hollow"></i>已存在</span>';
    case 'cancelled': return '<span class="st skip"><i class="dot hollow"></i>已取消</span>';
    default: return '<span class="st"><i class="dot hollow"></i>等待</span>';
  }
}
function badgeHTML(it) {
  const f = it.format || (it.outPath ? (it.outPath.split('.').pop() || '') : '');
  if (!f) return '<span class="badge">—</span>';
  const k = f.toLowerCase();
  const cls = ['flac', 'mp3', 'ogg', 'wav'].includes(k) ? k : '';
  return `<span class="badge ${cls}">${f.toUpperCase()}</span>`;
}

function createRow(it) {
  const el = document.createElement('div');
  el.className = 'row';
  el.dataset.path = it.path;
  el.innerHTML = `
    <span class="c-name"><span class="txt"></span></span>
    <span class="c-fmt">${badgeHTML(it)}</span>
    <span class="c-lyr">—</span>
    <span class="c-size num">—</span>
    <span class="c-status"></span>
    <span class="c-time num"></span>`;
  el.querySelector('.c-name .txt').textContent = it.name;
  el.querySelector('.c-status').innerHTML = statusHTML(it);
  el.addEventListener('click', () => openDetail(it.path));
  it.el = el;
  rowsEl.appendChild(el);
  return el;
}

function updateRow(it) {
  if (!it.el) return;
  it.el.classList.toggle('failed', it.status === 'failed');
  it.el.classList.toggle('current', S.currentId === it.path);
  it.el.querySelector('.c-fmt').innerHTML = badgeHTML(it);
  it.el.querySelector('.c-size').textContent = fmtSize(it.size);
  it.el.querySelector('.c-status').innerHTML = statusHTML(it);
  it.el.querySelector('.c-time').textContent = it.ms ? (it.ms / 1000).toFixed(2) + 's' : '';
  const lyr = it.el.querySelector('.c-lyr');
  if (it.hasLrc === undefined) { lyr.textContent = '—'; lyr.className = 'c-lyr miss'; }
  else if (!it.hasLrc) { lyr.textContent = '—'; lyr.className = 'c-lyr miss'; }
  else { lyr.textContent = 'LRC'; lyr.className = 'c-lyr' + (it.lrcCopied ? ' on' : ''); }
  applyFilterTo(it);
}

function applyFilterTo(it) {
  if (!it.el) return;
  const f = S.filter;
  let show = true;
  if (f === 'run') show = it.status === 'running';
  else if (f === 'fail') show = it.status === 'failed';
  else if (f === 'done') show = it.status === 'done';
  else if (f === 'skip') show = it.status === 'skipped' || it.status === 'cancelled';
  if (show && S.q) show = it.name.toLowerCase().includes(S.q.toLowerCase());
  it.el.style.display = show ? '' : 'none';
}

function counts() {
  let done = 0, fail = 0, skip = 0, run = 0, lrc = 0;
  for (const p of S.order) {
    const it = S.items.get(p);
    if (it.status === 'done') { done++; if (it.lrcCopied) lrc++; }
    else if (it.status === 'failed') fail++;
    else if (it.status === 'skipped' || it.status === 'cancelled') skip++;
    else if (it.status === 'running') run++;
  }
  return { done, fail, skip, run, lrc, total: S.order.length };
}

function updateAll() {
  const c = counts();
  $('#cAll').textContent = c.total;
  $('#cRun').textContent = c.run;
  $('#cFail').textContent = c.fail;
  $('#cDone').textContent = c.done;
  $('#cSkip').textContent = c.skip;

  $('#alert').classList.toggle('show', c.fail > 0);
  $('#alertN').textContent = c.fail;
  $('#btnRetryAll').disabled = c.fail === 0;
  $('#btnClear').disabled = c.total === 0 || S.running;

  const processed = c.done + c.fail + c.skip;
  const pct = c.total ? Math.round(processed / c.total * 100) : 0;
  $('#progBar').style.width = pct + '%';
  $('#sDone').textContent = c.done;
  $('#sFail').textContent = c.fail;
  $('#sSkip').textContent = c.skip;
  $('#sLrc').textContent = c.lrc;
  $('#sLrcStat').style.display = c.lrc ? 'flex' : 'none';
  $('#sPct').textContent = pct + '%';
  $('#sCount').textContent = processed + ' / ' + c.total;
  $('#sEta').textContent = S.running
    ? (c.total - processed > 0 ? '转换中…' : '收尾中…')
    : (processed === 0 ? '尚未开始' : (processed === c.total ? '全部完成' : '已停止'));

  emptyEl.style.display = c.total ? 'none' : 'grid';
}

/* ---------------- 加入文件 ---------------- */
/** 从扩展名猜格式(qmc 系列能猜,但最终以解密后的文件头为准) */
const QMC_EXT_RE = /\.(qmc0|qmc2|qmc3|qmcflac|qmcogg|qmc|mgg|mgg0|mgg1|mggl|mflac|mflac0|mflach|kwm)$/i;
function guessFormat(name) {
  const n = name.toLowerCase();
  if (/\.(flac)$/.test(n) || /\.(qmcflac|mflac|mflac0|mflach)$/.test(n)) return 'flac';
  if (/\.(qmcogg|mgg|mgg0|mgg1|mggl|qmc2)$/.test(n)) return 'ogg';
  if (/\.(qmc0|qmc3|qmc)$/.test(n)) return 'mp3';
  return null;
}

function addPaths(paths) {
  let n = 0;
  for (const p of paths) {
    if (!p || S.items.has(p)) continue;
    const name = p.split(/[\\/]/).pop();
    if (!/\.ncm$/i.test(name) && !QMC_EXT_RE.test(name)) continue;
    const it = { path: p, name, status: 'pending', size: 0, meta: null, outPath: null, err: null, ms: 0, format: guessFormat(name) };
    S.items.set(p, it);
    S.order.push(p);
    createRow(it);
    n++;
  }
  if (n) {
    updateAll();
    refreshSizes();
    scheduleSaveQueue();
  }
  return n;
}

async function refreshSizes() {
  const missing = S.order.map(p => S.items.get(p)).filter(i => !i.size).map(i => i.path);
  if (!missing.length) return;
  try {
    const list = await window.api.stat(missing);
    for (const { path, size } of list) {
      const it = S.items.get(path);
      if (it) { it.size = size; updateRow(it); }
    }
  } catch (_) {}
}

/* ---------------- 转换 ---------------- */
function syncStartBtn() {
  $('#startLabel').textContent = S.running ? '暂停' : '开始';
  $('#startIcon').innerHTML = S.running
    ? '<path d="M5 3h2.2v10H5zM8.8 3H11v10H8.8z"/>'
    : '<path d="M5 3.2l8 4.8-8 4.8z"/>';
}

async function startBatch() {
  if (S.running) return;
  const pending = S.order.map(p => S.items.get(p)).filter(i => i.status === 'pending');
  if (!pending.length) { toast('没有待处理的文件'); return; }

  S.running = true;
  syncStartBtn();
  updateAll();

  const res = await window.api.start({
    items: pending.map(i => i.path),
    outDir: S.outDir,
    copyLrc: S.copyLrc && !!S.outDir,
    template: S.template,
    overwrite: S.overwrite,
    concurrency: S.concurrency || undefined,
  });
  if (!res.ok) {
    S.running = false;
    syncStartBtn();
    toast(res.error || '启动失败');
  }
}

/* ---------------- 详情 ---------------- */
function openDetail(path) {
  const it = S.items.get(path);
  if (!it) return;
  S.currentId = path;
  document.querySelectorAll('.row.current').forEach(n => n.classList.remove('current'));
  it.el && it.el.classList.add('current');
  $('#content').classList.add('withdetail');

  const m = it.meta || {};
  const artist = Array.isArray(m.artist) ? m.artist.map(a => (Array.isArray(a) ? a[0] : a)).join(' / ') : '';
  const err = it.err
    ? `<div class="errcard"><b>${errLabel(it.err)} <span class="code">${it.err.code}</span></b>${escapeHtml(it.err.message || '')}</div>`
    : '';

  const outPath = it.outPath || '';
  const outDir = outPath ? outPath.replace(/[\\/][^\\/]*$/, '') : '';
  const outName = outPath ? outPath.slice(outDir.length).replace(/^[\\/]/, '') : '';

  $('#detail').innerHTML = `
    <div class="dhead"><b>${escapeHtml(it.name)}</b><button class="iconbtn" id="closeDetail" title="关闭" style="width:28px;height:28px;border-radius:5px">✕</button></div>
    <div class="cover"><svg viewBox="0 0 24 24"><circle cx="12" cy="12" r="8.5"/><circle cx="12" cy="12" r="2.2"/></svg></div>
    ${err}
    <div class="dsec"><h4>元数据</h4>
      <div class="kv"><span>歌名</span><span>${escapeHtml(m.musicName || '—')}</span></div>
      <div class="kv"><span>歌手</span><span>${escapeHtml(artist || '—')}</span></div>
      <div class="kv"><span>专辑</span><span>${escapeHtml(m.album || '—')}</span></div>
    </div>
    <div class="dsec"><h4>文件</h4>
      <div class="kv"><span>源文件</span><span class="ptxt" id="pSrc" title="${escapeHtml(it.path)}">${escapeHtml(it.path.split(/[\\/]/).pop() || '—')}</span></div>
      <div class="kv"><span>输出文件</span><span class="ptxt" id="pOutName" title="${escapeHtml(outPath)}">${escapeHtml(outName || '—')}</span></div>
      <div class="kv"><span>所在目录</span><span class="ptxt" id="pOutDir" title="${escapeHtml(outDir)}">${escapeHtml(outDir || '—')}</span></div>
      <div class="kv"><span>大小</span><span>${fmtSize(it.size)}</span></div>
      <div class="kv"><span>耗时</span><span>${it.ms ? (it.ms / 1000).toFixed(2) + ' 秒' : '—'}</span></div>
    </div>
    <div class="dact">
      <button class="btn" id="dPlay" ${it.outPath ? '' : 'disabled'}>试听</button>
      <button class="btn" id="dReveal">打开所在文件夹</button>
      <button class="btn" id="dRetry" ${it.status === 'failed' ? '' : 'disabled'}>重试</button>
    </div>`;

  $('#closeDetail').onclick = () => {
    $('#content').classList.remove('withdetail');
    S.currentId = null;
    it.el && it.el.classList.remove('current');
  };
  $('#dReveal').onclick = () => window.api.reveal(it.outPath || it.path);
  $('#dPlay').onclick = () => playTrack(it);

  // 点路径即复制
  const bindCopy = (sel, text) => {
    const el = $(sel);
    if (el && text) el.onclick = () => copyText(text);
  };
  bindCopy('#pSrc', it.path);
  bindCopy('#pOutName', outPath);
  bindCopy('#pOutDir', outDir);
  $('#dRetry').onclick = () => {
    it.status = 'pending'; it.err = null; it.ms = 0;
    updateRow(it); updateAll(); startBatch();
  };
}

const escapeHtml = (s) => String(s == null ? '' : s).replace(/[&<>"']/g, c => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' }[c]));

/* ---------------- 输出目录 ---------------- */
let popOut = null;
function applyOutDir(p) {
  if (!p) return;
  S.outDir = p;
  const label = p.split(/[\\/]/).filter(Boolean).pop() || p;
  $('#outLabel').textContent = label;
  $('#outLabel').title = p;
  syncLrcState();
}

/** 校验/创建目录;失败给出可读提示 */
async function safeEnsure(dir) {
  if (!dir) return null;
  if (!window.api.ensureDir) return dir;
  try {
    return await window.api.ensureDir(dir);
  } catch (e) {
    toast(String(e).replace(/^Error:\s*/, ''));
    return null;
  }
}

function buildPop() {
  popOut = document.createElement('div');
  popOut.className = 'pop wide';
  popOut.innerHTML = `
    <button class="item" data-k="same">同源目录</button>
    <button class="item" data-k="pick">选择目录…</button>
    <button class="item" data-k="manual">手动输入路径…</button>
    <div class="manual" id="manualBox" style="display:none">
      <input id="manualPath" placeholder="例如 F:\\dataCenter\\音乐" spellcheck="false">
      <button id="manualOk">确定</button>
    </div>`;
  document.body.appendChild(popOut);

  // 点弹层内部不要冒泡到 document(否则会被关掉)
  popOut.addEventListener('click', (e) => e.stopPropagation());

  popOut.addEventListener('click', async (e) => {
    if (e.target.closest('#manualOk')) {
      const v = popOut.querySelector('#manualPath').value.trim();
      const checked = await safeEnsure(v);
      if (checked) {
        applyOutDir(checked);
        popOut.classList.remove('show');
        toast('输出目录已设为 ' + checked);
      }
      return;
    }
    const b = e.target.closest('.item');
    if (!b) return;

    if (b.dataset.k === 'same') {
      S.outDir = null;
      $('#outLabel').textContent = '同源目录';
      $('#outLabel').removeAttribute('title');
      syncLrcState();
      popOut.classList.remove('show');
    } else if (b.dataset.k === 'pick') {
      popOut.classList.remove('show');
      const dir = await window.api.pickOutDir();
      if (dir) {
        const checked = await safeEnsure(dir);
        if (checked) applyOutDir(checked);
      }
    } else if (b.dataset.k === 'manual') {
      const box = popOut.querySelector('#manualBox');
      box.style.display = 'flex';
      popOut.querySelector('#manualPath').focus();
    }
  });
}

function syncLrcState() {
  const on = S.copyLrc;
  $('#chkLrc').classList.toggle('on', on);
  $('#lrcWrap').classList.toggle('off', !on);
  const label = $('#lrcLabel');
  if (label) label.textContent = on ? '同时复制歌词' : '同时复制歌词（已关）';
  $('#lrcWrap').title = on
    ? '转换时把与 .ncm 同名的 .lrc 一起复制到输出目录'
    : '已关闭:不会复制任何歌词文件';
}

/* ---------------- 事件绑定 ---------------- */
function bind() {
  $('#btnMin').onclick = () => window.api.minimize();
  $('#btnMax').onclick = () => window.api.maximize();
  $('#btnClose').onclick = () => window.api.close();
  $('#pClose').onclick = () => closePlayer();

  $('#btnAddFiles').onclick = async () => {    const files = await window.api.pickFiles();
    const n = addPaths(files);
    if (n) toast(`已添加 ${n} 个文件`);
  };
  $('#btnAddFolder').onclick = async () => {
    const files = await window.api.pickFolder();
    const n = addPaths(files);
    if (n) toast(`已添加 ${n} 个文件`);
  };
  $('#btnStart').onclick = () => {
    if (S.running) { window.api.cancel(); toast('正在停止…'); return; }
    startBatch();
  };
  $('#btnRetryAll').onclick = $('#alertRetry').onclick = () => {
    let n = 0;
    for (const p of S.order) {
      const it = S.items.get(p);
      if (it.status === 'failed') { it.status = 'pending'; it.err = null; it.ms = 0; updateRow(it); n++; }
    }
    if (n) { updateAll(); scheduleSaveQueue(); startBatch(); }
  };
  $('#btnClear').onclick = clearList;
  $('#alertFilter').onclick = () => setFilter('fail');

  document.querySelectorAll('.chip').forEach(c => {
    c.onclick = () => setFilter(c.dataset.f);
  });

  $('#q').oninput = (e) => { S.q = e.target.value.trim(); S.order.forEach(p => applyFilterTo(S.items.get(p))); };

  $('#btnOut').onclick = (e) => {
    e.stopPropagation();
    if (!popOut) buildPop();
    const r = e.currentTarget.getBoundingClientRect();
    popOut.style.left = r.left + 'px';
    popOut.style.top = (r.bottom + 6) + 'px';
    popOut.classList.toggle('show');
  };
  document.addEventListener('click', () => popOut && popOut.classList.remove('show'));

  $('#lrcWrap').onclick = (e) => {
    e.preventDefault();
    S.copyLrc = !S.copyLrc;
    syncLrcState();
    toast(S.copyLrc ? '转换时会同时复制同名 .lrc' : '已关闭歌词复制');
  };

  // 屏蔽浏览器右键菜单(输入框里保留,方便右键粘贴路径)
  document.addEventListener('contextmenu', (e) => {
    const t = e.target;
    const editable = t && (t.tagName === 'INPUT' || t.tagName === 'TEXTAREA' || t.isContentEditable);
    if (!editable) e.preventDefault();
  });

  window.addEventListener('dragover', (e) => { e.preventDefault(); });
  if (window.api.onDrop) {
    // Tauri:拖拽由外壳接管,拿到的是真实路径
    window.api.onDrop(async (paths) => {
      const expanded = await window.api.scanPaths(paths);
      const n = addPaths(expanded);
      if (n) toast(`已添加 ${n} 个文件`);
    });
  } else {
    window.addEventListener('drop', async (e) => {
      e.preventDefault();
      const files = Array.from(e.dataTransfer.files || []);
      const paths = files.map(f => window.api.pathForFile(f)).filter(Boolean);
      if (!paths.length) return;
      const expanded = await window.api.scanPaths(paths);
      const n = addPaths(expanded);
      if (n) toast(`已添加 ${n} 个文件`);
    });
  }

  document.addEventListener('keydown', (e) => {
    if (e.target.matches('input')) return;
    if (e.code === 'Space') { e.preventDefault(); $('#btnStart').click(); }
    if (e.key === 'Escape') { $('#content').classList.remove('withdetail'); S.currentId = null; }
  });

  window.api.onStartOne(({ path }) => {
    const it = S.items.get(path);
    if (!it) return;
    it.status = 'running';
    it.t0 = Date.now();
    updateRow(it);
    updateAll();
  });

  window.api.onResult((r) => {
    const it = S.items.get(r.srcPath);
    if (!it) return;
    it.status = r.status;
    it.meta = r.meta || it.meta;
    it.outPath = r.outPath || null;
    it.err = r.error || null;
    it.ms = r.ms || 0;
    it.lrcCopied = !!r.lrcCopied;
    if (r.format) it.format = r.format;
    if (typeof r.hasLrc === 'boolean') it.hasLrc = r.hasLrc;
    if (r.status === 'done' && r.outPath) {
      const idx = Math.max(r.outPath.lastIndexOf('\\'), r.outPath.lastIndexOf('/'));
      if (idx > 0) lastOutDir = r.outPath.slice(0, idx);
    }
    updateRow(it);
    // 失败置顶
    if (r.status === 'failed' && it.el && it.el.parentNode === rowsEl) {
      const first = rowsEl.querySelector('.row');
      if (first && first !== it.el) rowsEl.insertBefore(it.el, first);
    }
    updateAll();
    scheduleSaveQueue();
  });

  window.api.onDone((sum) => {
    S.running = false;
    syncStartBtn();
    saveQueueNow();
    updateAll();
    const parts = [];
    if (sum.done) parts.push(`${sum.done} 个完成`);
    if (sum.failed) parts.push(`${sum.failed} 个失败`);
    if (sum.skipped) parts.push(`${sum.skipped} 个跳过`);
    if (sum.lrc) parts.push(`${sum.lrc} 个歌词`);
    else if (S.copyLrc && S.outDir && sum.done) parts.push('没有找到同名 .lrc');
    toast(parts.length ? '转换结束:' + parts.join('，') : '转换结束');
    if (S.openAfter && lastOutDir && window.api.openDir) window.api.openDir(lastOutDir);
  });
}

function setFilter(f) {
  S.filter = f;
  document.querySelectorAll('.chip').forEach(c => c.classList.toggle('on', c.dataset.f === f));
  S.order.forEach(p => applyFilterTo(S.items.get(p)));
}

let toastTimer = null;
function toast(msg) {
  const t = $('#toast');
  t.classList.remove('sticky', 'undo');
  t.textContent = msg;
  t.classList.add('show');
  clearTimeout(toastTimer);
  toastTimer = setTimeout(() => t.classList.remove('show'), 2400);
}

/** 常驻提示,点击才消失 */
function toastSticky(msg) {
  const t = $('#toast');
  clearTimeout(toastTimer);
  t.textContent = msg;
  t.classList.add('show', 'sticky');
  t.onclick = () => t.classList.remove('show', 'sticky');
}

/* ---- 试听播放器 ---- */
let playing = { url: null, path: null };

async function copyText(text) {
  try {
    await navigator.clipboard.writeText(text);
    toast('已复制路径');
  } catch (_) {
    const ta = document.createElement('textarea');
    ta.value = text;
    document.body.appendChild(ta);
    ta.select();
    try { document.execCommand('copy'); toast('已复制路径'); } catch (e) { toast('复制失败'); }
    ta.remove();
  }
}

async function playTrack(it) {
  if (!it || !it.outPath) {
    toast('转换完成后才能试听');
    return;
  }
  if (playing.path === it.outPath) {
    $('#playerBar').hidden = false;
    $('#pAudio').play().catch((e) => toast('播放失败:' + (e.message || e)));
    return;
  }
  try {
    const buf = await window.api.readAudio(it.outPath);
    if (!buf || !buf.byteLength) {
      toast('读取音频失败:文件为空或不可读');
      return;
    }
    const mimeMap = { mp3: 'audio/mpeg', flac: 'audio/flac', ogg: 'audio/ogg', wav: 'audio/wav' };
    const mime = mimeMap[(it.format || '').toLowerCase()] || 'audio/mpeg';
    const blob = new Blob([buf], { type: mime });
    if (playing.url) URL.revokeObjectURL(playing.url);
    playing.url = URL.createObjectURL(blob);
    playing.path = it.outPath;

    const audio = $('#pAudio');
    audio.src = playing.url;
    $('#pTitle').textContent = it.name;
    $('#pTitle').title = it.outPath;
    $('#playerBar').hidden = false;
    audio.play().catch((e) => toast('播放失败:' + (e.message || e)));
  } catch (e) {
    toast(String(e).replace(/^Error:\s*/, ''));
  }
}

function closePlayer() {
  const audio = $('#pAudio');
  audio.pause();
  audio.removeAttribute('src');
  audio.load();
  if (playing.url) URL.revokeObjectURL(playing.url);
  playing = { url: null, path: null };
  $('#playerBar').hidden = true;
}

/* ---- 清空列表(可撤销) ---- */
let undoSnapshot = null;

/* ---- 断点续跑:列表存盘 / 恢复 ---- */
let queueTimer = null;

function queueSnapshot() {
  return S.order.map((p) => {
    const it = S.items.get(p);
    const m = it.meta || {};
    const artist = Array.isArray(m.artist)
      ? m.artist.map((a) => (Array.isArray(a) ? a[0] : a)).filter(Boolean).join(' / ')
      : '';
    return {
      path: it.path,
      // 正在跑的行存成 pending:下次打开是要重做,不是"还在跑"
      status: it.status === 'running' ? 'pending' : it.status,
      out: it.outPath || null,
      format: it.format || null,
      title: m.musicName || it.title || '',
      artist,
      album: m.album || '',
      size: it.size || 0,
      ms: it.ms || 0,
      errorCode: it.err ? it.err.code : null,
      errorMsg: it.err ? it.err.message : null,
      lrcCopied: !!it.lrcCopied,
      hasLrc: !!it.hasLrc,
    };
  });
}

/** 立刻落盘(批量结束时用,避免防抖还没触发就被关掉) */
function saveQueueNow() {
  if (!window.api.saveQueue) return;
  clearTimeout(queueTimer);
  try {
    window.api.saveQueue(queueSnapshot()).catch(() => {});
  } catch (_) {}
}

/** 合并频繁变更,最多每 800ms 落一次盘 */
function scheduleSaveQueue() {
  if (!window.api.saveQueue) return;
  clearTimeout(queueTimer);
  queueTimer = setTimeout(() => saveQueueNow(), 800);
}

/** 启动时把上次的列表接回来 */
async function restoreQueue() {
  if (!window.api.loadQueue) return 0;
  let list = [];
  try {
    list = await window.api.loadQueue();
  } catch (_) {
    return 0;
  }
  if (!list || !list.length) return 0;

  let n = 0;
  for (const q of list) {
    if (!q || !q.path || S.items.has(q.path)) continue;
    const name = q.path.split(/[\\/]/).pop();
    const it = {
      path: q.path,
      name,
      status: q.status === 'running' || !q.status ? 'pending' : q.status,
      size: q.size || 0,
      meta:
        q.title || q.artist || q.album
          ? { musicName: q.title || '', artist: q.artist ? [[q.artist, 0]] : [], album: q.album || '' }
          : null,
      outPath: q.out || null,
      err: q.errorCode ? { code: q.errorCode, message: q.errorMsg || '' } : null,
      ms: q.ms || 0,
      format: q.format || guessFormat(name),
      lrcCopied: !!q.lrcCopied,
      hasLrc: !!q.hasLrc,
    };
    S.items.set(it.path, it);
    S.order.push(it.path);
    createRow(it);
    updateRow(it);
    n++;
  }
  if (n) {
    updateAll();
    const left = [...S.items.values()].filter((i) => i.status === 'pending').length;
    toast(left ? `已恢复 ${n} 个文件,其中 ${left} 个待处理` : `已恢复 ${n} 个文件`);
  }
  return n;
}

function clearList() {
  if (S.running) { toast('转换进行中,请先暂停'); return; }
  const n = S.order.length;
  if (!n) return;

  undoSnapshot = { items: new Map(S.items), order: [...S.order] };

  S.items.clear();
  S.order = [];
  S.currentId = null;
  rowsEl.querySelectorAll('.row').forEach(el => el.remove());
  $('#content').classList.remove('withdetail');
  updateAll();
  if (window.api.clearQueue) {
    try { window.api.clearQueue().catch(() => {}); } catch (_) {}
  }
  toastUndo(`已清空 ${n} 个文件`);
}

function restoreList() {
  if (!undoSnapshot) return;
  S.items = undoSnapshot.items;
  S.order = undoSnapshot.order;
  undoSnapshot = null;
  for (const p of S.order) {
    const it = S.items.get(p);
    it.el = null;
    createRow(it);
    updateRow(it);
  }
  updateAll();
  scheduleSaveQueue();
  toast('已恢复');
}

function toastUndo(msg, ms = 6000) {
  const t = $('#toast');
  clearTimeout(toastTimer);
  t.innerHTML = '';
  const span = document.createElement('span');
  span.textContent = msg;
  const btn = document.createElement('b');
  btn.textContent = '撤销';
  btn.className = 'undo';
  btn.onclick = () => { restoreList(); t.classList.remove('show', 'undo'); };
  t.append(span, btn);
  t.classList.add('show', 'undo');
  toastTimer = setTimeout(() => {
    t.classList.remove('show', 'undo');
    undoSnapshot = null;
  }, ms);
}

/* ---------------- 设置 ---------------- */
function applySettings(st) {
  settings = Object.assign({}, DEFAULT_SETTINGS, st || {});
  S.template = settings.template;
  S.overwrite = settings.overwrite;
  S.concurrency = settings.concurrency;
  S.openAfter = !!settings.openAfter;
  S.copyLrc = settings.copyLrc !== false;
  if (settings.outDir) {
    applyOutDir(settings.outDir);
  } else {
    S.outDir = null;
    $('#outLabel').textContent = '同源目录';
    $('#outLabel').removeAttribute('title');
  }
  syncLrcState();
}

function fillSettingsForm() {
  const auto = Math.max(1, Math.min(16, (navigator.hardwareConcurrency || 8) - 1));
  const t = draft.concurrency > 0 ? draft.concurrency : auto;
  $('#setTemplate').value = draft.template;
  $('#setOverwrite').value = draft.overwrite;
  $('#setOutDir').value = draft.outDir || '';
  $('#setThreads').value = t;
  $('#setThreadsVal').textContent = draft.concurrency > 0 ? String(t) : t + '（自动）';
  $('#setOpenAfter').classList.toggle('on', !!draft.openAfter);
  $('#setCopyLrc').classList.toggle('on', draft.copyLrc !== false);
}

function openSettings() {
  draft = Object.assign({}, settings);
  fillSettingsForm();
  $('#setMask').classList.add('show');
}
function closeSettings() { $('#setMask').classList.remove('show'); }

async function saveSettings() {
  draft.template = ($('#setTemplate').value || '').trim() || DEFAULT_SETTINGS.template;
  if (!draft.template.includes('{title}')) {
    toast('命名模板必须包含 {title},否则同名文件会挤在一起');
    $('#setTemplate').focus();
    return;
  }
  draft.concurrency = Number($('#setThreads').value) || 0;
  try {
    const saved = await window.api.saveSettings(draft);
    applySettings(saved);
    closeSettings();
    toast('设置已保存');
  } catch (e) {
    toast(String(e).replace(/^Error:\s*/, ''));
  }
}

function bindSettings() {
  $('#btnSettings').onclick = openSettings;
  $('#setClose').onclick = closeSettings;
  $('#setCancel').onclick = closeSettings;
  $('#setMask').onclick = (e) => { if (e.target.id === 'setMask') closeSettings(); };

  $('#setTemplate').oninput = (e) => { draft.template = e.target.value; };
  $('#setOverwrite').onchange = (e) => { draft.overwrite = e.target.value; };
  $('#setThreads').oninput = (e) => {
    draft.concurrency = Number(e.target.value);
    $('#setThreadsVal').textContent = String(draft.concurrency);
  };

  const pickDir = async () => {
    const dir = await window.api.pickOutDir();
    if (!dir) return;
    const ok = await safeEnsure(dir);
    if (ok) { draft.outDir = ok; $('#setOutDir').value = ok; }
  };
  $('#setPickDir').onclick = (e) => { e.stopPropagation(); pickDir(); };
  $('#setOutDir').onclick = pickDir;
  $('#setSameDir').onclick = (e) => {
    e.stopPropagation();
    draft.outDir = null;
    $('#setOutDir').value = '';
  };
  $('#setOpenAfter').onclick = () => {
    draft.openAfter = !draft.openAfter;
    $('#setOpenAfter').classList.toggle('on', draft.openAfter);
  };
  $('#setCopyLrc').onclick = () => {
    draft.copyLrc = draft.copyLrc === false;
    $('#setCopyLrc').classList.toggle('on', draft.copyLrc !== false);
  };
  $('#setReset').onclick = () => {
    draft = Object.assign({}, DEFAULT_SETTINGS);
    fillSettingsForm();
    toast('已恢复默认值,记得点保存');
  };
  $('#setSave').onclick = saveSettings;

  document.addEventListener('keydown', (e) => {
    if (e.key === 'Escape' && $('#setMask').classList.contains('show')) closeSettings();
  });
}

/* ---------------- 启动 ---------------- */
bind();
bindSettings();
syncLrcState();
syncStartBtn();
updateAll();

// 载入设置(失败就用默认值,不能因此起不来)
(async () => {
  try {
    if (!window.api.loadSettings) return;
    const st = await window.api.loadSettings();
    if (st) applySettings(st);
  } catch (_) {}
})();

// --settings 启动参数:进来自动打开设置(调试/截图用)
(async () => {
  try {
    if (!window.api.startupFlags) return;
    const f = await window.api.startupFlags();
    if (f && f.openSettings) openSettings();
  } catch (_) {}
})();

// 受限运行环境检测:从聊天窗口/沙箱打开时,程序只能写自身目录
(async () => {
  try {
    if (!window.api.envStatus) return;
    const st = await window.api.envStatus();
    if (st && st.restricted) {
      toastSticky('⚠ 当前是受限运行环境,无法写入任何目录。请关闭窗口,到「文件资源管理器」里双击 tuneport.exe 或 启动.bat 重新打开。');
    }
  } catch (_) {}
})();

// 断点续跑:把上次的列表接回来(放在启动路径之后,避免重复)
(async () => {
  try {
    await restoreQueue();
  } catch (_) {}
})();

// 把文件/文件夹拖到 exe 图标上启动时,自动加载
(async () => {
  try {
    if (!window.api.startupPaths) return;
    const paths = await window.api.startupPaths();
    if (!paths || !paths.length) return;
    const expanded = await window.api.scanPaths(paths);
    const n = addPaths(expanded);
    if (n) toast(`已添加 ${n} 个文件`);
  } catch (_) {}
})();

// 供冒烟测试/调试使用
// 供冒烟测试/调试使用
window.__tuneport = {
  addPaths,
  start: startBatch,
  restoreQueue,
  setOut(dir) {
    if (dir) applyOutDir(dir);
    else {
      S.outDir = null;
      $('#outLabel').textContent = '同源目录';
      syncLrcState();
    }
  },
  summary() {
    const c = counts();
    const done = S.order.map(p => S.items.get(p)).filter(i => i.status === 'done');
    return { ...c, rows: rowsEl.querySelectorAll('.row').length, sample: done[0] ? done[0].outPath : null };
  },
};
