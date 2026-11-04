'use strict';
/** 测试用的 ncm 样本构造器(仅供测试,不参与打包) */
const crypto = require('crypto');

const CORE_KEY = Buffer.from('687a4852416d736f356b496e62617857', 'hex');
const META_KEY = Buffer.from('2331346C6A6B5F215C5D2630553C2728', 'hex');
const KEY_PREFIX = Buffer.from('neteasecloudmusic');
const META_PREFIX = Buffer.from("163 key(Don't modify):");

function aesEnc(key, data) {
  const c = crypto.createCipheriv('aes-128-ecb', key, null);
  return Buffer.concat([c.update(data), c.final()]);
}

function keystream(key) {
  const box = new Uint8Array(256);
  for (let i = 0; i < 256; i++) box[i] = i;
  let j = 0;
  for (let i = 0; i < 256; i++) {
    j = (box[i] + j + key[i % key.length]) & 0xff;
    const t = box[i]; box[i] = box[j]; box[j] = t;
  }
  const ks = Buffer.alloc(256);
  for (let i = 0; i < 256; i++) {
    const idx = (i + 1) & 0xff;
    const si = box[idx];
    const sj = box[(idx + si) & 0xff];
    ks[i] = box[(si + sj) & 0xff];
  }
  return ks;
}

function buildNcm({ audio, keyPlain, meta, layout = 'A', imageLen = 2048 }) {
  const keyCipher = Buffer.from(aesEnc(CORE_KEY, keyPlain)).map(b => b ^ 0x64);
  const metaCipher = aesEnc(META_KEY, Buffer.from('music:' + JSON.stringify(meta), 'utf8'));
  const metaSection = Buffer.concat([META_PREFIX, Buffer.from(metaCipher.toString('base64'), 'ascii')])
    .map(b => b ^ 0x63);

  const parts = [Buffer.from('CTENFDAM', 'ascii'), Buffer.from([0x01, 0x70])];
  const klen = Buffer.alloc(4); klen.writeUInt32LE(keyCipher.length); parts.push(klen, keyCipher);
  const mlen = Buffer.alloc(4); mlen.writeUInt32LE(metaSection.length); parts.push(mlen, metaSection);

  const crc = Buffer.alloc(4);
  const image = Buffer.alloc(imageLen);
  image[0] = 0xff; image[1] = 0xd8; image[2] = 0xff; image[3] = 0xe0;
  const ilen = Buffer.alloc(4); ilen.writeUInt32LE(image.length);

  if (layout === 'A') parts.push(crc, Buffer.from([0x01]), ilen, image);
  else if (layout === 'B') parts.push(crc, Buffer.alloc(5), ilen, image);
  else parts.push(crc, Buffer.from([0x01]), ilen, Buffer.alloc(4), image);

  const ks = keystream(keyPlain.subarray(17));
  const enc = Buffer.from(audio);
  for (let i = 0; i < enc.length; i++) enc[i] ^= ks[i & 0xff];
  parts.push(enc);
  return Buffer.concat(parts);
}

function fakeFlac(frames) {
  const streaminfo = Buffer.alloc(34);
  streaminfo.writeUInt16BE(4096, 0);
  streaminfo.writeUInt16BE(4096, 2);
  return Buffer.concat([
    Buffer.from('fLaC', 'ascii'),
    Buffer.from([0x80, 0x00, 0x00, 34]),   // 唯一块 → last
    streaminfo,
    frames,
  ]);
}

function fakeMp3(frames) {
  return Buffer.concat([Buffer.from([0xff, 0xfb, 0x90, 0x00]), frames]);
}

function makeSample(overrides = {}) {
  const keyPlain = Buffer.concat([KEY_PREFIX, crypto.randomBytes(16)]);
  const meta = Object.assign({
    musicId: 1,
    musicName: '夏日信笺',
    artist: [['林墨', 1], ['苏晚', 2]],
    album: '白日梦游',
    format: 'flac',
    duration: 215000,
  }, overrides.meta || {});
  const audio = overrides.audio || fakeFlac(crypto.randomBytes(30000));
  const layout = overrides.layout || 'A';
  return { buf: buildNcm({ audio, keyPlain, meta, layout }), audio, meta };
}

module.exports = { buildNcm, fakeFlac, fakeMp3, keystream, makeSample, CORE_KEY, META_KEY, KEY_PREFIX, META_PREFIX };
