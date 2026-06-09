#!/usr/bin/env node
// Dependency-free icon generator for the XenoClaw desktop app.
//
// Renders a Claude-Desktop-style app mark — a warm coral rounded square with a
// cream "spark" sunburst — at every size Tauri needs, plus a multi-image
// Windows .ico. Uses only Node built-ins (zlib for PNG compression), so it runs
// anywhere without ImageMagick/Inkscape/PIL installed.
//
// Usage: node scripts/generate-icons.mjs
// Output: web/src-tauri/icons/*.png + icon.ico
//
// It also prints a structurally-valid *placeholder* updater public key so the
// Tauri config parses; replace it with a real key from `npm run tauri signer
// generate` before shipping production auto-updates.

import { deflateSync } from 'node:zlib';
import { writeFileSync, mkdirSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { dirname, join } from 'node:path';
import { generateKeyPairSync, randomBytes } from 'node:crypto';

const __dirname = dirname(fileURLToPath(import.meta.url));
const ICON_DIR = join(__dirname, '..', 'icons');
mkdirSync(ICON_DIR, { recursive: true });

// ---- Palette (Claude Desktop) ----------------------------------------------
const CORAL = [0xd9, 0x77, 0x57]; // primary accent
const CORAL_DEEP = [0xc2, 0x5b, 0x3c];
const CREAM = [0xfa, 0xf9, 0xf5];

// ---- CRC32 (for PNG/ICO chunks) --------------------------------------------
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

// ---- Minimal PNG encoder (RGBA, 8-bit) -------------------------------------
function chunk(type, data) {
  const len = Buffer.alloc(4);
  len.writeUInt32BE(data.length, 0);
  const typeBuf = Buffer.from(type, 'ascii');
  const crc = Buffer.alloc(4);
  crc.writeUInt32BE(crc32(Buffer.concat([typeBuf, data])), 0);
  return Buffer.concat([len, typeBuf, data, crc]);
}
function encodePng(width, height, rgba) {
  const sig = Buffer.from([137, 80, 78, 71, 13, 10, 26, 10]);
  const ihdr = Buffer.alloc(13);
  ihdr.writeUInt32BE(width, 0);
  ihdr.writeUInt32BE(height, 4);
  ihdr[8] = 8; // bit depth
  ihdr[9] = 6; // colour type RGBA
  ihdr[10] = 0; ihdr[11] = 0; ihdr[12] = 0;
  // Prepend a 0 (no-filter) byte to each scanline.
  const stride = width * 4;
  const raw = Buffer.alloc((stride + 1) * height);
  for (let y = 0; y < height; y++) {
    raw[y * (stride + 1)] = 0;
    rgba.copy(raw, y * (stride + 1) + 1, y * stride, y * stride + stride);
  }
  const idat = deflateSync(raw, { level: 9 });
  return Buffer.concat([sig, chunk('IHDR', ihdr), chunk('IDAT', idat), chunk('IEND', Buffer.alloc(0))]);
}

// ---- Vector-ish rasteriser (supersampled) ----------------------------------
// Rounded-square signed distance (negative inside).
function sdRoundRect(px, py, cx, cy, halfW, halfH, r) {
  const qx = Math.abs(px - cx) - (halfW - r);
  const qy = Math.abs(py - cy) - (halfH - r);
  const ax = Math.max(qx, 0);
  const ay = Math.max(qy, 0);
  return Math.hypot(ax, ay) + Math.min(Math.max(qx, qy), 0) - r;
}

function renderRgba(size) {
  const ss = 4; // supersample factor
  const S = size * ss;
  const acc = new Float32Array(S * S * 4);
  const c = S / 2;
  const sq = S * 0.46; // half side of the rounded square
  const radius = S * 0.225;
  const rays = 8;
  const innerR = S * 0.05;
  const outerR = S * 0.34;
  const rayHalf = S * 0.052; // ray half-thickness

  for (let y = 0; y < S; y++) {
    for (let x = 0; x < S; x++) {
      const px = x + 0.5;
      const py = y + 0.5;
      let r = 0, g = 0, b = 0, a = 0;

      // Background rounded square with a soft vertical coral gradient.
      const d = sdRoundRect(px, py, c, c, sq, sq, radius);
      if (d < 0) {
        const t = py / S; // 0 top -> 1 bottom
        r = CORAL[0] * (1 - t) + CORAL_DEEP[0] * t;
        g = CORAL[1] * (1 - t) + CORAL_DEEP[1] * t;
        b = CORAL[2] * (1 - t) + CORAL_DEEP[2] * t;
        a = 255;
      }

      // Cream sunburst on top.
      const dx = px - c;
      const dy = py - c;
      const dist = Math.hypot(dx, dy);
      let spark = false;
      if (dist < innerR * 2.0) spark = true; // centre nub
      if (dist >= innerR && dist <= outerR) {
        const ang = Math.atan2(dy, dx);
        for (let k = 0; k < rays; k++) {
          const ra = (k / rays) * Math.PI * 2;
          // Perpendicular distance from the ray axis.
          const along = dx * Math.cos(ra) + dy * Math.sin(ra);
          const perp = -dx * Math.sin(ra) + dy * Math.cos(ra);
          if (along > 0) {
            // Taper the ray toward its tip for a petal-like look.
            const taper = rayHalf * (1 - (along / outerR) * 0.55);
            if (Math.abs(perp) <= taper && along <= outerR) {
              spark = true;
              break;
            }
          }
          void ang;
        }
      }
      if (spark && a > 0) {
        r = CREAM[0]; g = CREAM[1]; b = CREAM[2]; a = 255;
      }

      const idx = (y * S + x) * 4;
      acc[idx] = r; acc[idx + 1] = g; acc[idx + 2] = b; acc[idx + 3] = a;
    }
  }

  // Box-downsample to the target size.
  const out = Buffer.alloc(size * size * 4);
  for (let y = 0; y < size; y++) {
    for (let x = 0; x < size; x++) {
      let r = 0, g = 0, b = 0, a = 0;
      for (let sy = 0; sy < ss; sy++) {
        for (let sx = 0; sx < ss; sx++) {
          const idx = (((y * ss + sy) * S) + (x * ss + sx)) * 4;
          r += acc[idx]; g += acc[idx + 1]; b += acc[idx + 2]; a += acc[idx + 3];
        }
      }
      const n = ss * ss;
      const o = (y * size + x) * 4;
      out[o] = Math.round(r / n);
      out[o + 1] = Math.round(g / n);
      out[o + 2] = Math.round(b / n);
      out[o + 3] = Math.round(a / n);
    }
  }
  return out;
}

function pngForSize(size) {
  return encodePng(size, size, renderRgba(size));
}

// ---- ICO (PNG-compressed entries, Vista+) ----------------------------------
function buildIco(sizes) {
  const images = sizes.map((s) => ({ size: s, png: pngForSize(s) }));
  const header = Buffer.alloc(6);
  header.writeUInt16LE(0, 0); // reserved
  header.writeUInt16LE(1, 2); // type: icon
  header.writeUInt16LE(images.length, 4);
  const dir = Buffer.alloc(16 * images.length);
  let offset = 6 + dir.length;
  images.forEach((img, i) => {
    const e = i * 16;
    dir[e] = img.size >= 256 ? 0 : img.size; // width (0 == 256)
    dir[e + 1] = img.size >= 256 ? 0 : img.size; // height
    dir[e + 2] = 0; // palette
    dir[e + 3] = 0; // reserved
    dir.writeUInt16LE(1, e + 4); // colour planes
    dir.writeUInt16LE(32, e + 6); // bits per pixel
    dir.writeUInt32LE(img.png.length, e + 8);
    dir.writeUInt32LE(offset, e + 12);
    offset += img.png.length;
  });
  return Buffer.concat([header, dir, ...images.map((i) => i.png)]);
}

// ---- Emit ------------------------------------------------------------------
const pngSizes = {
  '32x32.png': 32,
  '128x128.png': 128,
  '128x128@2x.png': 256,
  'icon.png': 512,
  'Square30x30Logo.png': 30,
  'Square44x44Logo.png': 44,
  'Square71x71Logo.png': 71,
  'Square89x89Logo.png': 89,
  'Square107x107Logo.png': 107,
  'Square142x142Logo.png': 142,
  'Square150x150Logo.png': 150,
  'Square284x284Logo.png': 284,
  'Square310x310Logo.png': 310,
  'StoreLogo.png': 50,
};
for (const [name, size] of Object.entries(pngSizes)) {
  writeFileSync(join(ICON_DIR, name), pngForSize(size));
}
writeFileSync(join(ICON_DIR, 'icon.ico'), buildIco([16, 24, 32, 48, 64, 128, 256]));
console.log(`Wrote ${Object.keys(pngSizes).length} PNG icons + icon.ico to ${ICON_DIR}`);

// ---- Placeholder updater pubkey --------------------------------------------
// 42-byte minisign-style blob: b"Ed" + 8-byte key id + 32-byte ed25519 pubkey.
try {
  const { publicKey } = generateKeyPairSync('ed25519');
  const der = publicKey.export({ type: 'spki', format: 'der' });
  const ed = der.subarray(der.length - 32); // raw 32-byte key trails the SPKI header
  const blob = Buffer.concat([Buffer.from('Ed', 'ascii'), randomBytes(8), ed]);
  console.log('\nPlaceholder updater pubkey (replace before shipping updates):');
  console.log(blob.toString('base64'));
} catch (e) {
  console.warn('Could not generate placeholder updater key:', e.message);
}
