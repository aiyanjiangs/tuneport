'use strict';
/** 用 jsdom 在无桌面环境里测界面逻辑:加文件 → 事件回填 → 失败置顶 → 详情面板 */
const fs = require('fs');
const path = require('path');
const { JSDOM } = require('jsdom');

let pass = 0, fail = 0;
function ok(c, name, extra) {
  if (c) { pass++; console.log('  ✓ ' + name); }
  else { fail++; console.log('  ✗ ' + name + (extra ? '  → ' + extra : '')); }
}
const tick = () => new Promise(r => setTimeout(r, 10));

(async () => {
  const root = path.join(__dirname, '..');
  const html = fs.readFileSync(path.join(root, 'renderer', 'index.html'), 'utf8');
  const appSrc = fs.readFileSync(path.join(root, 'renderer', 'app.js'), 'utf8');

  const dom = new JSDOM(html, { runScripts: 'outside-only', url: 'file:///app/index.html' });
  const { window } = dom;
  const doc = window.document;

  const L = {};
  window.api = {
    pickFiles: async () => [], pickFolder: async () => [], pickOutDir: async () => null,
    stat: async (paths) => paths.map(p => ({ path: p, size: 1048576 * 30 })),
    scanPaths: async (p) => p,
    start: async (payload) => { window.__lastStart = payload; return { ok: true }; },
    cancel: async () => { window.__cancelled = true; },
    reveal: async () => {}, openPath: async () => {},
    minimize: async () => {}, maximize: async () => {}, close: async () => {},
    onStartOne: cb => { L.start = cb; },
    onResult: cb => { L.result = cb; },
    onDone: cb => { L.done = cb; },
  };

  window.eval(appSrc);
  await tick();

  console.log('\n[1] 加入文件');
  const n = window.__tuneport.addPaths(['C:\\m\\a.ncm', 'C:\\m\\b.ncm', 'C:\\m\\skip.txt']);
  ok(n === 2, '只收 .ncm,加了 2 个', String(n));
  ok(doc.querySelectorAll('.row').length === 2, '渲染出 2 行');
  ok(doc.querySelector('#sCount').textContent === '0 / 2', '计数 0 / 2', doc.querySelector('#sCount').textContent);
  await tick();
  ok(doc.querySelector('.row .c-size').textContent.includes('MB'), '大小已填充');

  console.log('\n[2] 点击开始');
  await window.__tuneport.start();
  ok(window.__lastStart && window.__lastStart.items.length === 2, '把 2 个路径交给主进程');
  ok(window.__lastStart.outDir === null, '默认输出到同源目录');
  ok(window.__lastStart.copyLrc === false, '同源目录时默认不复制歌词');
  ok(doc.querySelector('#startLabel').textContent === '暂停', '按钮变为「暂停」');

  console.log('\n[3] 回填结果');
  L.start({ path: 'C:\\m\\a.ncm' });
  ok(doc.querySelector('.row .c-status').textContent.includes('转换中'), '状态变为转换中');

  L.result({
    srcPath: 'C:\\m\\a.ncm', status: 'done', outPath: 'C:\\m\\林墨 - 夏日信笺.flac',
    format: 'flac', hasLrc: true, lrcCopied: true, ms: 1200,
    meta: { musicName: '夏日信笺', artist: [['林墨', 1]], album: '白日梦游' },
  });
  L.result({ srcPath: 'C:\\m\\b.ncm', status: 'failed', error: { code: 'E_NO_KEY', message: '文件不含密钥' }, ms: 0 });
  await tick();

  const rows = [...doc.querySelectorAll('.row')];
  ok(rows[0].textContent.includes('b.ncm'), '失败行被置顶');
  ok(rows[0].classList.contains('failed'), '失败行有 failed 样式');
  ok(rows[0].textContent.includes('需密钥'), '失败原因显示为「需密钥」');
  ok(doc.querySelector('#sDone').textContent === '1', '完成计数 1');
  ok(doc.querySelector('#sFail').textContent === '1', '失败计数 1');
  ok(doc.querySelector('#alert').classList.contains('show'), '顶部失败提示条出现');

  const doneRow = rows.find(r => r.textContent.includes('a.ncm'));
  ok(doneRow.textContent.includes('FLAC'), '完成行显示 FLAC 徽标');
  ok(doneRow.querySelector('.c-lyr').classList.contains('on'), '歌词列高亮');

  console.log('\n[4] 输出目录与歌词开关');
  window.__tuneport.setOut('C:\\converted');
  ok(doc.querySelector('#outLabel').textContent === 'converted', '输出目录标签更新');
  ok(!doc.querySelector('#lrcWrap').classList.contains('disabled'), '歌词勾选变为可用');
  doc.querySelector('#lrcWrap').click();
  ok(!doc.querySelector('#chkLrc').classList.contains('on'), '再次点击取消勾选');

  console.log('\n[5] 筛选与详情');
  doc.querySelector('.chip[data-f="fail"]').click();
  const visible = [...doc.querySelectorAll('.row')].filter(r => r.style.display !== 'none');
  ok(visible.length === 1 && visible[0].textContent.includes('b.ncm'), '失败筛选只剩 1 行');

  doc.querySelector('.chip[data-f="all"]').click();
  const aRow = [...doc.querySelectorAll('.row')].find(r => r.textContent.includes('a.ncm'));
  aRow.click();
  ok(doc.querySelector('#content').classList.contains('withdetail'), '详情面板打开');
  ok(doc.querySelector('#detail').textContent.includes('夏日信笺'), '详情显示歌名');
  ok(doc.querySelector('#detail').textContent.includes('白日梦游'), '详情显示专辑');

  console.log('\n[6] 结束');
  L.done({ total: 2, done: 1, failed: 1, skipped: 0, lrc: 1 });
  await tick();
  ok(doc.querySelector('#startLabel').textContent === '开始', '按钮恢复为「开始」');
  ok(doc.querySelector('#sEta').textContent === '全部完成', '状态栏显示全部完成', doc.querySelector('#sEta').textContent);

  console.log(`\n结果:${pass} 通过,${fail} 失败`);
  process.exit(fail ? 1 : 0);
})().catch(e => { console.error('测试异常:', e); process.exit(2); });
