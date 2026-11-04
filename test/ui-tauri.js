'use strict';
/** 验证 Tauri 适配层:invoke 的命令名/参数形状、事件字段映射 */
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

  const dom = new JSDOM(html, { runScripts: 'outside-only', url: 'tauri://localhost/' });
  const { window } = dom;
  const doc = window.document;

  const calls = [];
  const listeners = {};
  let unlisten = 0;

  let queueItems = [];
  window.__TAURI__ = {
    core: {
      invoke: async (cmd, args) => {
        calls.push({ cmd, args });
        if (cmd === 'stat_files') return (args.paths || []).map(p => ({ path: p, size: 1024 * 1024 * 12 }));
        if (cmd === 'scan_paths') return args.paths || [];
        if (cmd === 'ensure_dir') return args.path;
        if (cmd === 'load_settings') {
          return { template: '{artist} - {title}', overwrite: 'rename', concurrency: 0, outDir: null, openAfter: false, copyLrc: true };
        }
        if (cmd === 'save_settings') return args.settings;
        if (cmd === 'load_queue') return queueItems;
        if (cmd === 'save_queue' || cmd === 'clear_queue') return null;
        return null;
      },
    },
    event: {
      listen: async (name, cb) => { listeners[name] = cb; return () => { unlisten++; }; },
    },
  };

  window.eval(appSrc);
  await tick();

  console.log('\n[1] 适配层注册');
  ok(!!window.api, 'window.api 已由 __TAURI__ 生成');
  ok(typeof window.api.onDrop === 'function', '注册了 Tauri 的拖拽事件');
  ok(!!listeners['task-result'] && !!listeners['batch-done'], '监听了 task-result / batch-done');

  console.log('\n[2] 命令名与参数形状');
  const n = window.__tuneport.addPaths(['C:\\m\\a.ncm']);
  ok(n === 1, '加了一个文件');
  await tick();
  const stat = calls.find(c => c.cmd === 'stat_files');
  ok(!!stat && Array.isArray(stat.args.paths), 'stat_files 收到 { paths: [...] }', JSON.stringify(stat && stat.args));

  await window.__tuneport.start();
  await tick();
  const startCall = calls.find(c => c.cmd === 'start_batch');
  ok(!!startCall, '调用了 start_batch');
  ok(startCall && startCall.args && startCall.args.args, '参数嵌套在 { args: {...} } 里(匹配 Rust 命令签名)',
    JSON.stringify(startCall && startCall.args && Object.keys(startCall.args)));
  ok(startCall && Array.isArray(startCall.args.args.items), 'items 是数组');
  ok(startCall && startCall.args.args.outDir === null, '默认 outDir 为 null');

  console.log('\n[3] 事件字段映射');
  listeners['task-result']({
    payload: {
      src: 'C:\\m\\a.ncm', status: 'done', out: 'C:\\m\\林墨 - 夏日信笺.flac',
      format: 'flac', title: '夏日信笺', artist: '林墨', album: '白日梦游',
      hasLrc: true, lrcCopied: true, ms: 860,
      errorCode: null, errorMsg: null,
    },
  });
  await tick();

  const row = doc.querySelector('.row');
  ok(row.textContent.includes('完成'), '状态显示完成');
  ok(row.textContent.includes('FLAC'), 'FLAC 徽标');
  ok(doc.querySelector('#sDone').textContent === '1', '完成计数 1');
  ok(doc.querySelector('#lrcWrap') !== null, '界面元素正常');

  doc.querySelector('.row').click();
  const detail = doc.querySelector('#detail').textContent;
  ok(detail.includes('夏日信笺'), '详情显示歌名(来自 title 字段)');
  ok(detail.includes('林墨'), '详情显示歌手(来自 artist 字段)');
  ok(detail.includes('林墨 - 夏日信笺.flac'), '详情显示输出文件名');
  ok(detail.includes('C:\\m'), '详情显示输出所在目录');

  console.log('\n[4] 失败事件映射');
  window.__tuneport.addPaths(['C:\\m\\b.ncm']);
  await tick();
  listeners['task-result']({
    payload: {
      src: 'C:\\m\\b.ncm', status: 'failed', out: null, format: null,
      title: '', artist: '', album: '', hasLrc: false, lrcCopied: false, ms: 5,
      errorCode: 'E_FORMAT', errorMsg: '不是 ncm 文件(魔数不匹配)',
    },
  });
  await tick();
  const rows = [...doc.querySelectorAll('.row')];
  const failRow = rows.find(r => r.textContent.includes('b.ncm'));
  ok(!!failRow && failRow.classList.contains('failed'), '失败行标红');
  ok(failRow && failRow.textContent.includes('格式不支持'), '错误码映射成中文标签', failRow && failRow.textContent);
  ok(rows[0].textContent.includes('b.ncm'), '失败行置顶');

  console.log('\n[5] 输出目录:手动输入路径');
  doc.querySelector('#btnOut').click();
  await tick();
  ok(!!doc.querySelector('#manualPath'), '菜单里有手动输入框');
  doc.querySelector('.item[data-k="manual"]').click();
  await tick();
  ok(doc.querySelector('#manualBox').style.display === 'flex', '点「手动输入路径」展开输入框');

  doc.querySelector('#manualPath').value = 'F:\\dataCenter\\音乐';
  doc.querySelector('#manualOk').click();
  await tick();
  await tick();
  ok(doc.querySelector('#outLabel').textContent === '音乐', '输出目录标签更新为末级目录', doc.querySelector('#outLabel').textContent);
  ok(!doc.querySelector('#lrcWrap').classList.contains('disabled'), '歌词勾选随之可用');
  const ensureCall = calls.find(c => c.cmd === 'ensure_dir');
  ok(!!ensureCall && ensureCall.args.path === 'F:\\dataCenter\\音乐', '调用了 ensure_dir 校验目录可写');

  // 歌词开关:默认开,点一下关,再点开
  ok(doc.querySelector('#lrcLabel').textContent === '同时复制歌词', '歌词开关默认开启', doc.querySelector('#lrcLabel').textContent);
  doc.querySelector('#lrcWrap').click();
  await tick();
  ok(doc.querySelector('#lrcLabel').textContent.includes('已关'), '点击后显示已关闭');
  doc.querySelector('#lrcWrap').click();
  await tick();
  ok(doc.querySelector('#lrcLabel').textContent === '同时复制歌词', '再点一次恢复开启');

  console.log('\n[6] batch-done');
  listeners['batch-done']({ payload: { total: 2, done: 1, failed: 1, skipped: 0, lrc: 1, ms: 900 } });
  await tick();
  ok(doc.querySelector('#startLabel').textContent === '开始', '按钮恢复');

  console.log('\n[7] 清空列表与撤销');
  const before = doc.querySelectorAll('.row').length;
  doc.querySelector('#btnClear').click();
  await tick();
  ok(doc.querySelectorAll('.row').length === 0, '清空后没有任何行');
  ok(doc.querySelector('#sCount').textContent === '0 / 0', '计数归零', doc.querySelector('#sCount').textContent);
  ok(doc.querySelector('#empty').style.display !== 'none', '空状态出现');
  const undo = doc.querySelector('#toast .undo');
  ok(!!undo, '提示里有撤销按钮');
  undo.click();
  await tick();
  ok(doc.querySelectorAll('.row').length === before, '撤销后恢复原来的行数', String(doc.querySelectorAll('.row').length));

  console.log('\n[8] 设置弹窗');
  doc.querySelector('#btnSettings').click();
  await tick();
  ok(doc.querySelector('#setMask').classList.contains('show'), '点齿轮打开设置弹窗');

  doc.querySelector('#setTemplate').value = '{album}';
  doc.querySelector('#setSave').click();
  await tick();
  await tick();
  ok(doc.querySelector('#setMask').classList.contains('show'), '模板不含 {title} 时拒绝保存并保持打开');
  ok(!calls.some(c => c.cmd === 'save_settings'), '非法模板没有发出保存请求');

  doc.querySelector('#setTemplate').value = '{artist} - {title} - {album}';
  doc.querySelector('#setThreads').value = '4';
  doc.querySelector('#setSave').click();
  await tick();
  await tick();
  const saveCall = calls.find(c => c.cmd === 'save_settings');
  ok(!!saveCall, '调用了 save_settings');
  ok(saveCall && saveCall.args.settings.template === '{artist} - {title} - {album}', '模板已提交', saveCall && saveCall.args.settings.template);
  ok(saveCall && saveCall.args.settings.concurrency === 4, '并发数已提交', saveCall && saveCall.args.settings.concurrency);
  ok(!doc.querySelector('#setMask').classList.contains('show'), '保存成功后弹窗关闭');

  console.log('\n[9] 断点续跑:恢复上次的列表');
  queueItems = [
    { path: 'C:\\m\\old1.ncm', status: 'done', out: 'C:\\m\\A - B.flac', format: 'flac', title: 'B', artist: 'A', album: '', size: 2048, ms: 300, lrcCopied: false, hasLrc: false },
    { path: 'C:\\m\\old2.kwm', status: 'failed', errorCode: 'E_KEY', errorMsg: '密钥错误', size: 4096, ms: 0, title: '', artist: '', album: '', lrcCopied: false, hasLrc: false },
    { path: 'C:\\m\\old3.mflac', status: 'running', size: 1024, ms: 0, title: '', artist: '', album: '', lrcCopied: false, hasLrc: false },
  ];
  const restoredCount = await window.__tuneport.restoreQueue();
  await tick();
  ok(restoredCount === 3, '恢复了 3 个文件', restoredCount);
  const rowsNow = [...doc.querySelectorAll('.row')].map(r => r.textContent);
  const restored = rowsNow.filter(t => t.includes('old1.ncm') || t.includes('old2.kwm') || t.includes('old3.mflac'));
  ok(restored.length === 3, '上次的 3 个文件已恢复', rowsNow.join(' | ').slice(0, 200));
  ok(rowsNow.some(t => t.includes('old1.ncm') && t.includes('完成')), '已完成的行恢复成"完成"');
  ok(rowsNow.some(t => t.includes('old2.kwm') && t.includes('密钥错误')), '失败的行保留了错误原因');
  ok(rowsNow.some(t => t.includes('old3.mflac') && t.includes('等待')), '中断时"进行中"的行恢复成"等待"');

  console.log('\n[10] 清空列表要同时清掉存盘');
  doc.querySelector('#btnClear').click();
  await tick();
  ok(calls.some(c => c.cmd === 'clear_queue'), '清空时调用了 clear_queue');

  console.log(`\n结果:${pass} 通过,${fail} 失败`);
  process.exit(fail ? 1 : 0);
})().catch(e => { console.error('测试异常:', e); process.exit(2); });
