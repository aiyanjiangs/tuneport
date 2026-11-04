'use strict';
/** 生成 Tauri 需要的图标(纯手写 PNG/ICO,不装依赖) */
const fs = require('fs');
const path = require('path');
const zlib = require('zlib');

const outDir = path.join(__dirname, '..', 'src-tauri', 'icons');
fs.mkdirSync(outDir, { recursive: true });

function crc32(buf) {
  let c, crc = 0xffffffff;
  for (let i = 0; i < buf.length; i++) {
    c = (crc ^ buf[i]) & 0xff;
    for (let k = 0; k < 8; k++) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1;
    crc = c ^ (crc >>> 8);
  }
  return (crc ^ 0xffffffff) >>> 0;
}

function chunk(type, data) {
  const len = Buffer.alloc(4);
  len.writeUInt32BE(data.length);
  const t = Buffer.from(type, 'ascii');
  const body = Buffer.concat([t, data]);
  const crc = Buffer.alloc(4);
  crc.writeUInt32BE(crc32(body));
  return Buffer.concat([len, body, crc]);
}

/** 画一个圆角方块 + 三条竖线(波形),返回 RGBA 像素 */
function drawIcon(size) {
  const px = Buffer.alloc(size * size * 4);
  const r = size * 0.22;
  const set = (x, y, [R, G, B, A]) => {
    const i = (y * size + x) * 4;
    px[i] = R; px[i + 1] = G; px[i + 2] = B; px[i + 3] = A;
  };
  for (let y = 0; y < size; y++) {
    for (let x = 0; x < size; x++) {
      // 圆角矩形
      const dx = Math.max(r - x, 0, x - (size - 1 - r));
      const dy = Math.max(r - y, 0, y - (size - 1 - r));
      const inside = dx * dx + dy * dy <= r * r;
      if (!inside) { set(x, y, [0, 0, 0, 0]); continue; }
      // 渐变:左上亮蓝 → 右下深蓝
      const t = (x / size) * 0.5 + (y / size) * 0.5;
      set(x, y, [
        Math.round(0x2f + (0x00 - 0x2f) * t),
        Math.round(0x8b + (0x55 - 0x8b) * t),
        Math.round(0xff + (0xb8 - 0xff) * t),
        255,
      ]);
    }
  }
  // 三条白色竖条
  const bars = [[0.30, 0.42, 0.28], [0.47, 0.62, 0.30], [0.64, 0.40, 0.26]];
  for (const [bx, bh] of bars) {
    const w = Math.max(2, Math.round(size * 0.09));
    const h = Math.round(size * bh);
    const x0 = Math.round(size * bx);
    const y0 = Math.round((size - h) / 2);
    for (let y = y0; y < y0 + h; y++) {
      for (let x = x0; x < x0 + w; x++) {
        if (x >= 0 && x < size && y >= 0 && y < size) set(x, y, [255, 255, 255, 255]);
      }
    }
  }
  return px;
}

function png(size) {
  const rgba = drawIcon(size);
  const raw = Buffer.alloc((size * 4 + 1) * size);
  for (let y = 0; y < size; y++) {
    raw[y * (size * 4 + 1)] = 0; // filter: none
    rgba.copy(raw, y * (size * 4 + 1) + 1, y * size * 4, (y + 1) * size * 4);
  }
  const ihdr = Buffer.alloc(13);
  ihdr.writeUInt32BE(size, 0);
  ihdr.writeUInt32BE(size, 4);
  ihdr[8] = 8;   // bit depth
  ihdr[9] = 6;   // RGBA
  return Buffer.concat([
    Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]),
    chunk('IHDR', ihdr),
    chunk('IDAT', zlib.deflateSync(raw, { level: 9 })),
    chunk('IEND', Buffer.alloc(0)),
  ]);
}

function ico(pngs) {
  const header = Buffer.alloc(6);
  header.writeUInt16LE(0, 0);
  header.writeUInt16LE(1, 2);       // type: icon
  header.writeUInt16LE(pngs.length, 4);
  const entries = [];
  let offset = 6 + 16 * pngs.length;
  for (const { size, data } of pngs) {
    const e = Buffer.alloc(16);
    e[0] = size >= 256 ? 0 : size;
    e[1] = size >= 256 ? 0 : size;
    e[2] = 0; e[3] = 0;
    e.writeUInt16LE(1, 4);          // planes
    e.writeUInt16LE(32, 6);         // bpp
    e.writeUInt32LE(data.length, 8);
    e.writeUInt32LE(offset, 12);
    offset += data.length;
    entries.push(e);
  }
  return Buffer.concat([header, ...entries, ...pngs.map(p => p.data)]);
}

const files = {};
for (const size of [32, 128, 256]) {
  const buf = png(size);
  files[size] = buf;
  const name = size === 256 ? '128x128@2x.png' : `${size}x${size}.png`;
  fs.writeFileSync(path.join(outDir, name), buf);
}
fs.writeFileSync(path.join(outDir, 'icon.png'), files[256]);
fs.writeFileSync(path.join(outDir, 'icon.ico'), ico([
  { size: 32, data: files[32] },
  { size: 128, data: files[128] },
  { size: 256, data: files[256] },
]));
console.log('icons written to', outDir, fs.readdirSync(outDir).join(', '));
