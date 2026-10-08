// Generates a simple source PNG icon (1024x1024) for NClientV4.
// Run with: `node scripts/make-icon.mjs` then
// `npm run tauri icon src-tauri/icons/icon.png` to produce all formats.
//
// This script emits a minimal valid PNG by hand so it has no dependencies.

import { writeFileSync, mkdirSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import zlib from "node:zlib";

const __dirname = dirname(fileURLToPath(import.meta.url));
const outDir = join(__dirname, "..", "src-tauri", "icons");
mkdirSync(outDir, { recursive: true });

const SIZE = 1024;

// Build RGBA pixels: dark background + a stylised "N".
const bg = [14, 15, 19, 255]; // #0e0f13
const accent = [91, 140, 255, 255]; // #5b8cff
const accent2 = [58, 102, 230, 255]; // #3a66e6

const px = new Uint8Array(SIZE * SIZE * 4);
for (let y = 0; y < SIZE; y++) {
  for (let x = 0; x < SIZE; x++) {
    const i = (y * SIZE + x) * 4;
    let color = bg;
    // Rounded square background (lighter).
    const inset = 80;
    if (x > inset && x < SIZE - inset && y > inset && y < SIZE - inset) {
      color = [23, 25, 34, 255];
    }
    // Draw an "N" shape using simple rectangles.
    const nx0 = SIZE * 0.30;
    const nx1 = SIZE * 0.70;
    const ny0 = SIZE * 0.25;
    const ny1 = SIZE * 0.75;
    const stroke = SIZE * 0.085;
    // Left vertical bar.
    if (x >= nx0 && x < nx0 + stroke && y >= ny0 && y < ny1) color = accent;
    // Right vertical bar.
    if (x >= nx1 - stroke && x < nx1 && y >= ny0 && y < ny1) color = accent;
    // Diagonal.
    const t = (x - nx0) / (nx1 - nx0);
    const diagY = ny0 + t * (ny1 - ny0);
    if (Math.abs(y - diagY) < stroke / 2 && x >= nx0 && x < nx1) color = accent2;

    px[i] = color[0];
    px[i + 1] = color[1];
    px[i + 2] = color[2];
    px[i + 3] = color[3];
  }
}

function chunk(type, data) {
  const len = Buffer.alloc(4);
  len.writeUInt32BE(data.length, 0);
  const typeBuf = Buffer.from(type, "ascii");
  const crc = Buffer.alloc(4);
  crc.writeUInt32BE(crc32(Buffer.concat([typeBuf, data])), 0);
  return Buffer.concat([len, typeBuf, data, crc]);
}

const CRC_TABLE = (() => {
  const t = new Uint32Array(256);
  for (let n = 0; n < 256; n++) {
    let c = n;
    for (let k = 0; k < 8; k++) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1;
    t[n] = c >>> 0;
  }
  return t;
})();
function crc32(buf) {
  let c = 0xffffffff;
  for (let i = 0; i < buf.length; i++) c = CRC_TABLE[(c ^ buf[i]) & 0xff] ^ (c >>> 8);
  return (c ^ 0xffffffff) >>> 0;
}

// Build raw scanlines with filter byte 0 prefix per row.
const raw = Buffer.alloc((SIZE * 4 + 1) * SIZE);
for (let y = 0; y < SIZE; y++) {
  raw[y * (SIZE * 4 + 1)] = 0;
  Buffer.from(px.buffer, y * SIZE * 4, SIZE * 4).copy(raw, y * (SIZE * 4 + 1) + 1);
}
const idat = zlib.deflateSync(raw, { level: 9 });

const ihdr = Buffer.alloc(13);
ihdr.writeUInt32BE(SIZE, 0);
ihdr.writeUInt32BE(SIZE, 4);
ihdr[8] = 8; // bit depth
ihdr[9] = 6; // color type RGBA
ihdr[10] = 0;
ihdr[11] = 0;
ihdr[12] = 0;

const png = Buffer.concat([
  Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]),
  chunk("IHDR", ihdr),
  chunk("IDAT", idat),
  chunk("IEND", Buffer.alloc(0)),
]);

writeFileSync(join(outDir, "icon.png"), png);
console.log(`Wrote ${outDir}/icon.png (${png.length} bytes).`);
console.log("Next: npm run tauri icon src-tauri/icons/icon.png");
