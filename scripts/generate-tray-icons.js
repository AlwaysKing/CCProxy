// 生成 macOS 托盘模板图标（纯黑 + alpha，系统自动反色适配深浅色）。
// 输出 build/icons/tray{Running,Stopped}Template.png 与 @2x。手写 PNG，零依赖。
// 用法：node scripts/generate-tray-icons.js

'use strict';

const fs = require('fs');
const path = require('path');
const zlib = require('zlib');

function crc32(buf) {
  let c;
  let crc = 0xffffffff;
  for (let i = 0; i < buf.length; i++) {
    c = (crc ^ buf[i]) & 0xff;
    for (let k = 0; k < 8; k++) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1;
    crc = c ^ (crc >>> 8);
  }
  return (crc ^ 0xffffffff) >>> 0;
}

function chunk(type, data) {
  const length = Buffer.alloc(4);
  length.writeUInt32BE(data.length);
  const body = Buffer.concat([Buffer.from(type, 'ascii'), data]);
  const crc = Buffer.alloc(4);
  crc.writeUInt32BE(crc32(body));
  return Buffer.concat([length, body, crc]);
}

// 8 位 RGBA（色彩类型 6），每行前置一个 0 滤波器字节
function makePng(size, alphaAt) {
  const ihdr = Buffer.alloc(13);
  ihdr.writeUInt32BE(size, 0);
  ihdr.writeUInt32BE(size, 4);
  ihdr[8] = 8;
  ihdr[9] = 6;
  const raw = Buffer.alloc(size * (size * 4 + 1));
  let cursor = 0;
  for (let y = 0; y < size; y++) {
    raw[cursor++] = 0;
    for (let x = 0; x < size; x++) {
      raw[cursor++] = 0;
      raw[cursor++] = 0;
      raw[cursor++] = 0;
      raw[cursor++] = alphaAt(x + 0.5, y + 0.5);
    }
  }
  return Buffer.concat([
    Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]),
    chunk('IHDR', ihdr),
    chunk('IDAT', zlib.deflateSync(raw, { level: 9 })),
    chunk('IEND', Buffer.alloc(0))
  ]);
}

// 圆形边缘 1px 抗锯齿
function circleAlpha(distance, radius) {
  if (distance <= radius - 0.5) return 255;
  if (distance >= radius + 0.5) return 0;
  return Math.round(255 * (radius + 0.5 - distance));
}

// innerRadius > 0 时挖成空心圆环（停止态）
function alphaFor(distance, spec) {
  if (spec.innerRadius > 0 && distance < spec.innerRadius) return 0;
  return circleAlpha(distance, spec.radius);
}

const SPECS = [
  { name: 'trayRunningTemplate.png', size: 16, radius: 6.2, innerRadius: 0 },
  { name: 'trayRunningTemplate@2x.png', size: 32, radius: 12.4, innerRadius: 0 },
  { name: 'trayStoppedTemplate.png', size: 16, radius: 6.2, innerRadius: 3.6 },
  { name: 'trayStoppedTemplate@2x.png', size: 32, radius: 12.4, innerRadius: 7.2 }
];

const outDir = path.join(__dirname, '..', 'build', 'icons');
fs.mkdirSync(outDir, { recursive: true });

for (const spec of SPECS) {
  const center = spec.size / 2;
  const png = makePng(spec.size, (x, y) => alphaFor(Math.hypot(x - center, y - center), spec));
  fs.writeFileSync(path.join(outDir, spec.name), png);
  console.log(`${spec.name}  ${png.length} bytes`);
}
