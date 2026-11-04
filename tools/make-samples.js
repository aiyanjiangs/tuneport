'use strict';
/** 生成几个测试用 ncm 到指定目录:node tools/make-samples.js <目录> */
const fs = require('fs');
const path = require('path');
const crypto = require('crypto');
const fixture = require('../test/fixture');

const dir = process.argv[2] || 'tmp-samples';
fs.mkdirSync(dir, { recursive: true });

const samples = [
  { name: 'song-a.ncm', s: fixture.makeSample({ layout: 'A' }) },
  {
    name: 'song-b.ncm',
    s: fixture.makeSample({
      layout: 'B',
      audio: fixture.fakeMp3(crypto.randomBytes(15000)),
      meta: { musicName: '北方的雨', artist: [['陈屿', 9]], album: '海岸线', format: 'mp3' },
    }),
  },
  { name: 'song-c.ncm', s: fixture.makeSample({ layout: 'C' }) },
];

for (const { name, s } of samples) {
  fs.writeFileSync(path.join(dir, name), s.buf);
}
fs.writeFileSync(path.join(dir, 'song-a.lrc'), '[00:01.00]测试歌词\n');
console.log('created', samples.length, 'samples in', path.resolve(dir));
