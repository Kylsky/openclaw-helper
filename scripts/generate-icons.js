const fs = require("node:fs/promises");
const fssync = require("node:fs");
const path = require("node:path");
const zlib = require("node:zlib");
const { execFileSync } = require("node:child_process");

const SRGB_TO_LINEAR_LUT = Array.from({ length: 256 }, (_v, i) => srgbToLinear(i));

function clamp(value, min, max) {
  return Math.max(min, Math.min(max, value));
}

function smoothstep(edge0, edge1, x) {
  const t = clamp((x - edge0) / (edge1 - edge0), 0, 1);
  return t * t * (3 - 2 * t);
}

function mix(a, b, t) {
  return a + (b - a) * t;
}

function srgbToLinear(u8) {
  const x = u8 / 255;
  return x <= 0.04045 ? x / 12.92 : Math.pow((x + 0.055) / 1.055, 2.4);
}

function linearToSrgb01(x) {
  const v = clamp(x, 0, 1);
  return v <= 0.0031308 ? v * 12.92 : 1.055 * Math.pow(v, 1 / 2.4) - 0.055;
}

function resizeRgbaBilinear({ srcRgba, srcWidth, srcHeight, dstWidth, dstHeight }) {
  const dst = Buffer.alloc(dstWidth * dstHeight * 4);
  const scaleX = srcWidth / dstWidth;
  const scaleY = srcHeight / dstHeight;

  for (let y = 0; y < dstHeight; y++) {
    const sy = (y + 0.5) * scaleY - 0.5;
    const y0 = Math.floor(sy);
    const y1 = y0 + 1;
    const ty = sy - y0;

    const yy0 = clamp(y0, 0, srcHeight - 1);
    const yy1 = clamp(y1, 0, srcHeight - 1);

    for (let x = 0; x < dstWidth; x++) {
      const sx = (x + 0.5) * scaleX - 0.5;
      const x0 = Math.floor(sx);
      const x1 = x0 + 1;
      const tx = sx - x0;

      const xx0 = clamp(x0, 0, srcWidth - 1);
      const xx1 = clamp(x1, 0, srcWidth - 1);

      const w00 = (1 - tx) * (1 - ty);
      const w10 = tx * (1 - ty);
      const w01 = (1 - tx) * ty;
      const w11 = tx * ty;

      let accA = 0;
      let accR = 0;
      let accG = 0;
      let accB = 0;

      const sample = (sx0, sy0, w) => {
        if (w === 0) return;
        const idx = (sy0 * srcWidth + sx0) * 4;
        const a = srcRgba[idx + 3] / 255;
        const r = SRGB_TO_LINEAR_LUT[srcRgba[idx]] * a;
        const g = SRGB_TO_LINEAR_LUT[srcRgba[idx + 1]] * a;
        const b = SRGB_TO_LINEAR_LUT[srcRgba[idx + 2]] * a;
        accA += a * w;
        accR += r * w;
        accG += g * w;
        accB += b * w;
      };

      sample(xx0, yy0, w00);
      sample(xx1, yy0, w10);
      sample(xx0, yy1, w01);
      sample(xx1, yy1, w11);

      let outR = 0;
      let outG = 0;
      let outB = 0;

      if (accA > 0) {
        outR = accR / accA;
        outG = accG / accA;
        outB = accB / accA;
      }

      const di = (y * dstWidth + x) * 4;
      dst[di] = Math.round(clamp(linearToSrgb01(outR), 0, 1) * 255);
      dst[di + 1] = Math.round(clamp(linearToSrgb01(outG), 0, 1) * 255);
      dst[di + 2] = Math.round(clamp(linearToSrgb01(outB), 0, 1) * 255);
      dst[di + 3] = Math.round(clamp(accA, 0, 1) * 255);
    }
  }

  return dst;
}

function writeUInt32BE(buffer, offset, value) {
  buffer.writeUInt32BE(value >>> 0, offset);
}

function crc32(buffer) {
  let crc = 0xffffffff;
  for (let i = 0; i < buffer.length; i++) {
    crc ^= buffer[i];
    for (let k = 0; k < 8; k++) {
      const mask = -(crc & 1);
      crc = (crc >>> 1) ^ (0xedb88320 & mask);
    }
  }
  return (crc ^ 0xffffffff) >>> 0;
}

function pngChunk(type, data) {
  const typeBuf = Buffer.from(type, "ascii");
  const lengthBuf = Buffer.alloc(4);
  writeUInt32BE(lengthBuf, 0, data.length);
  const crcBuf = Buffer.alloc(4);
  const crc = crc32(Buffer.concat([typeBuf, data]));
  writeUInt32BE(crcBuf, 0, crc);
  return Buffer.concat([lengthBuf, typeBuf, data, crcBuf]);
}

function encodePng({ width, height, rgba }) {
  if (rgba.length !== width * height * 4) {
    throw new Error("RGBA buffer size mismatch");
  }

  const signature = Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]);

  const ihdr = Buffer.alloc(13);
  writeUInt32BE(ihdr, 0, width);
  writeUInt32BE(ihdr, 4, height);
  ihdr[8] = 8; // bit depth
  ihdr[9] = 6; // color type RGBA
  ihdr[10] = 0; // compression
  ihdr[11] = 0; // filter
  ihdr[12] = 0; // interlace

  const scanlineBytes = width * 4;
  const raw = Buffer.alloc(height * (1 + scanlineBytes));
  for (let y = 0; y < height; y++) {
    raw[y * (1 + scanlineBytes)] = 0; // filter type 0
    rgba.copy(raw, y * (1 + scanlineBytes) + 1, y * scanlineBytes, (y + 1) * scanlineBytes);
  }

  const compressed = zlib.deflateSync(raw, { level: 9 });

  const idat = pngChunk("IDAT", compressed);
  const iend = pngChunk("IEND", Buffer.alloc(0));
  const ihdrChunk = pngChunk("IHDR", ihdr);

  return Buffer.concat([signature, ihdrChunk, idat, iend]);
}

function sdRoundBox(px, py, bx, by, radius) {
  const ax = Math.abs(px) - bx;
  const ay = Math.abs(py) - by;
  const qx = Math.max(ax, 0);
  const qy = Math.max(ay, 0);
  const outside = Math.hypot(qx, qy);
  const inside = Math.min(Math.max(ax, ay), 0);
  return outside + inside - radius;
}

function sdCapsule(px, py, ax, ay, bx, by, radius) {
  const pax = px - ax;
  const pay = py - ay;
  const bax = bx - ax;
  const bay = by - ay;
  const baLen2 = bax * bax + bay * bay;
  const h = baLen2 === 0 ? 0 : clamp((pax * bax + pay * bay) / baLen2, 0, 1);
  const dx = pax - bax * h;
  const dy = pay - bay * h;
  return Math.hypot(dx, dy) - radius;
}

function generateBaseIconRgba({ size }) {
  const width = size;
  const height = size;
  const rgba = Buffer.alloc(width * height * 4);

  const accent1 = [0x4f, 0x8c, 0xff];
  const accent2 = [0x8b, 0x5c, 0xf6];
  const a1 = accent1.map(srgbToLinear);
  const a2 = accent2.map(srgbToLinear);

  const aa = 2 / size;

  for (let y = 0; y < height; y++) {
    for (let x = 0; x < width; x++) {
      const nx = (x + 0.5) / width * 2 - 1;
      const ny = (y + 0.5) / height * 2 - 1;

      const d = sdRoundBox(nx, ny, 0.78, 0.78, 0.28);
      const mask = smoothstep(aa, -aa, d);

      const t = clamp((nx * 0.6 + ny * -0.8 + 1) / 2, 0, 1);
      const radial = clamp(1 - Math.hypot(nx * 0.9, ny * 0.9), 0, 1);
      const lift = radial * 0.12;

      const bg = [
        mix(a1[0], a2[0], t) + lift,
        mix(a1[1], a2[1], t) + lift,
        mix(a1[2], a2[2], t) + lift
      ].map((c) => clamp(c, 0, 1));

      const edge = smoothstep(0.06, 0.0, Math.abs(d));
      bg[0] += edge * 0.08;
      bg[1] += edge * 0.08;
      bg[2] += edge * 0.08;

      const s1 = sdCapsule(nx, ny, -0.42, -0.12, 0.06, 0.58, 0.055);
      const s2 = sdCapsule(nx, ny, -0.12, -0.34, 0.34, 0.36, 0.055);
      const s3 = sdCapsule(nx, ny, 0.18, -0.58, 0.62, 0.08, 0.055);

      const slashDist = Math.min(s1, s2, s3);
      const slash = smoothstep(aa * 1.2, -aa * 1.2, slashDist) * mask;

      const shadow = smoothstep(aa * 1.6, -aa * 1.6, slashDist + 0.018) * mask;

      const white = [1, 1, 1];
      bg[0] = mix(bg[0], white[0], slash * 0.92);
      bg[1] = mix(bg[1], white[1], slash * 0.92);
      bg[2] = mix(bg[2], white[2], slash * 0.92);

      bg[0] = mix(bg[0], bg[0] * 0.65, shadow * 0.35);
      bg[1] = mix(bg[1], bg[1] * 0.65, shadow * 0.35);
      bg[2] = mix(bg[2], bg[2] * 0.65, shadow * 0.35);

      const sr = Math.round(clamp(linearToSrgb01(bg[0]), 0, 1) * 255);
      const sg = Math.round(clamp(linearToSrgb01(bg[1]), 0, 1) * 255);
      const sb = Math.round(clamp(linearToSrgb01(bg[2]), 0, 1) * 255);
      const sa = Math.round(mask * 255);

      const idx = (y * width + x) * 4;
      rgba[idx] = sr;
      rgba[idx + 1] = sg;
      rgba[idx + 2] = sb;
      rgba[idx + 3] = sa;
    }
  }

  return rgba;
}

function ensureDir(dirPath) {
  if (!fssync.existsSync(dirPath)) {
    fssync.mkdirSync(dirPath, { recursive: true });
  }
}

async function writeFile(filePath, data) {
  await fs.mkdir(path.dirname(filePath), { recursive: true });
  await fs.writeFile(filePath, data);
}

function buildIcoFromPngBuffers(entries) {
  const header = Buffer.alloc(6);
  header.writeUInt16LE(0, 0); // reserved
  header.writeUInt16LE(1, 2); // type icon
  header.writeUInt16LE(entries.length, 4); // count

  const dir = Buffer.alloc(entries.length * 16);
  let offset = header.length + dir.length;

  const payloads = [];
  for (let i = 0; i < entries.length; i++) {
    const { size, png } = entries[i];
    const w = size === 256 ? 0 : size;
    const h = size === 256 ? 0 : size;

    const base = i * 16;
    dir[base + 0] = w;
    dir[base + 1] = h;
    dir[base + 2] = 0; // color count
    dir[base + 3] = 0; // reserved
    dir.writeUInt16LE(1, base + 4); // planes
    dir.writeUInt16LE(32, base + 6); // bit count
    dir.writeUInt32LE(png.length, base + 8); // bytes
    dir.writeUInt32LE(offset, base + 12); // offset

    offset += png.length;
    payloads.push(png);
  }

  return Buffer.concat([header, dir, ...payloads]);
}

async function generateIcons() {
  const root = path.join(__dirname, "..");
  const buildDir = path.join(root, "build");
  ensureDir(buildDir);

  const baseSize = 1024;
  const baseRgba = generateBaseIconRgba({ size: baseSize });
  const basePng = encodePng({ width: baseSize, height: baseSize, rgba: baseRgba });

  const basePngPath = path.join(buildDir, "icon.png");
  await writeFile(basePngPath, basePng);

  if (process.platform === "darwin") {
    const iconsetDir = path.join(buildDir, "icon.iconset");
    await fs.rm(iconsetDir, { recursive: true, force: true });
    await fs.mkdir(iconsetDir, { recursive: true });

    const iconsetFiles = [
      { name: "icon_16x16.png", size: 16 },
      { name: "icon_16x16@2x.png", size: 32 },
      { name: "icon_32x32.png", size: 32 },
      { name: "icon_32x32@2x.png", size: 64 },
      { name: "icon_128x128.png", size: 128 },
      { name: "icon_128x128@2x.png", size: 256 },
      { name: "icon_256x256.png", size: 256 },
      { name: "icon_256x256@2x.png", size: 512 },
      { name: "icon_512x512.png", size: 512 },
      { name: "icon_512x512@2x.png", size: 1024 }
    ];

    for (const { name, size } of iconsetFiles) {
      const rgba = size === baseSize
        ? baseRgba
        : resizeRgbaBilinear({
            srcRgba: baseRgba,
            srcWidth: baseSize,
            srcHeight: baseSize,
            dstWidth: size,
            dstHeight: size
          });
      const png = encodePng({ width: size, height: size, rgba });
      await writeFile(path.join(iconsetDir, name), png);
    }

    const icnsPath = path.join(buildDir, "icon.icns");
    execFileSync("/usr/bin/iconutil", ["-c", "icns", iconsetDir, "-o", icnsPath], { stdio: "ignore" });

    await fs.rm(iconsetDir, { recursive: true, force: true });
  }

  const icoSizes = [16, 32, 48, 64, 128, 256];
  const icoEntries = [];

  for (const size of icoSizes) {
    const rgba = size === baseSize
      ? baseRgba
      : resizeRgbaBilinear({
          srcRgba: baseRgba,
          srcWidth: baseSize,
          srcHeight: baseSize,
          dstWidth: size,
          dstHeight: size
        });
    const png = encodePng({ width: size, height: size, rgba });
    icoEntries.push({ size, png });
  }

  const ico = buildIcoFromPngBuffers(icoEntries);
  await writeFile(path.join(buildDir, "icon.ico"), ico);
}

generateIcons()
  .then(() => {
    process.stdout.write("Icons generated in ./build\n");
  })
  .catch((error) => {
    process.stderr.write(`${error?.stack || error}\n`);
    process.exitCode = 1;
  });
