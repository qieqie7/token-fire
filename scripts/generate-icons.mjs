#!/usr/bin/env node
import { execFileSync } from "node:child_process";
import { mkdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { deflateSync } from "node:zlib";
import { fileURLToPath } from "node:url";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const iconsDir = join(root, "src-tauri", "icons");
const appIconPath = join(iconsDir, "icon.png");
const trayIconPath = join(iconsDir, "tray-icon.png");
const icnsPath = join(iconsDir, "icon.icns");
const iconsetDir = join(tmpdir(), "tokenfire.iconset");

const A1 = {
  bgTop: hex("#262e38"),
  bgBottom: hex("#11151b"),
  fireStops: [
    [0, hex("#ff8a00")],
    [0.38, hex("#ff6d00")],
    [0.72, hex("#ff3d00")],
    [1, hex("#ff2d20")],
  ],
  coreStops: [
    [0, hex("#fff59d")],
    [0.55, hex("#ffd54f")],
    [1, hex("#ff9800")],
  ],
};

const outerFlame = [
  move(66, 110),
  curve(44, 110, 27, 95, 27, 73),
  curve(27, 56, 37, 45, 47, 35),
  curve(52, 29, 56, 23, 57, 15),
  curve(71, 25, 79, 36, 78, 50),
  curve(84, 46, 88, 40, 90, 32),
  curve(98, 41, 102, 52, 102, 64),
  curve(102, 92, 86, 110, 66, 110),
];

const innerFlame = [
  move(61, 103),
  curve(49, 103, 40, 95, 40, 83),
  curve(40, 74, 46, 68, 52, 62),
  curve(55, 59, 57, 55, 58, 50),
  curve(66, 56, 70, 62, 70, 70),
  curve(74, 67, 76, 64, 77, 60),
  curve(82, 66, 84, 72, 84, 79),
  curve(84, 94, 75, 103, 61, 103),
];

mkdirSync(iconsDir, { recursive: true });

function renderAppIcon(size) {
  const img = createImage(size, size);
  drawRoundedRectGradient(img, size * 0.235, A1.bgTop, A1.bgBottom);
  const outer = flattenPath(outerFlame, size / 128, 28);
  const inner = flattenPath(innerFlame, size / 128, 28);
  drawPolygonGradient(img, outer, A1.fireStops);
  drawPolygonGradient(img, inner, A1.coreStops);
  drawTopHighlight(img);
  return img;
}

function renderTrayIcon(size) {
  const img = createImage(size, size);
  const scale = size / 128;
  const trayOffsetY = -7;
  const outer = flattenPath(translatePath(outerFlame, 0, trayOffsetY), scale, 28);
  const inner = flattenPath(translatePath(innerFlame, 0, trayOffsetY), scale, 28);
  drawPolygonGradient(img, outer, A1.fireStops);
  drawPolygonGradient(img, inner, A1.coreStops);
  return img;
}

function buildIcns() {
  rmSync(iconsetDir, { force: true, recursive: true });
  mkdirSync(iconsetDir, { recursive: true });
  const sizes = [
    ["icp4", "icon_16x16.png", 16],
    ["icp5", "icon_32x32.png", 32],
    ["icp6", "icon_64x64.png", 64],
    ["ic07", "icon_128x128.png", 128],
    ["ic08", "icon_256x256.png", 256],
    ["ic09", "icon_512x512.png", 512],
    ["ic10", "icon_1024x1024.png", 1024],
  ];

  for (const [, name, size] of sizes) {
    execFileSync("sips", ["-z", String(size), String(size), appIconPath, "--out", join(iconsetDir, name)], {
      stdio: "ignore",
    });
  }
  writeIcns(
    icnsPath,
    sizes.map(([type, name]) => [type, readFileSync(join(iconsetDir, name))]),
  );
  rmSync(iconsetDir, { force: true, recursive: true });
}

function writeIcns(path, entries) {
  const totalLength = 8 + entries.reduce((sum, [, data]) => sum + 8 + data.length, 0);
  const out = Buffer.alloc(totalLength);
  out.write("icns", 0, "ascii");
  out.writeUInt32BE(totalLength, 4);
  let offset = 8;
  for (const [type, data] of entries) {
    out.write(type, offset, "ascii");
    out.writeUInt32BE(8 + data.length, offset + 4);
    data.copy(out, offset + 8);
    offset += 8 + data.length;
  }
  writeFileSync(path, out);
}

function createImage(width, height) {
  return { width, height, pixels: Buffer.alloc(width * height * 4) };
}

function drawRoundedRectGradient(img, radius, top, bottom) {
  const { width, height, pixels } = img;
  const cx = width / 2;
  const cy = height / 2;
  const halfW = width / 2;
  const halfH = height / 2;
  for (let y = 0; y < height; y += 1) {
    for (let x = 0; x < width; x += 1) {
      const px = x + 0.5;
      const py = y + 0.5;
      const dx = Math.max(Math.abs(px - cx) - (halfW - radius), 0);
      const dy = Math.max(Math.abs(py - cy) - (halfH - radius), 0);
      const dist = Math.hypot(dx, dy) - radius;
      const alpha = clamp(0.5 - dist, 0, 1);
      if (alpha <= 0) continue;
      const color = mix(top, bottom, y / Math.max(1, height - 1));
      blend(pixels, width, x, y, color, alpha);
    }
  }
}

function drawPolygonGradient(img, polygon, stops) {
  const { width, pixels } = img;
  const box = bounds(polygon, img.width, img.height);
  const height = Math.max(1, box.maxY - box.minY);
  for (let y = box.minY; y <= box.maxY; y += 1) {
    for (let x = box.minX; x <= box.maxX; x += 1) {
      if (!insidePolygon(x + 0.5, y + 0.5, polygon)) continue;
      const color = gradient(stops, (y - box.minY) / height);
      blend(pixels, width, x, y, color, 1);
    }
  }
}

function drawRoundedSegment(img, x1, y1, x2, y2, radius, scale, color, opacity) {
  x1 *= scale;
  y1 *= scale;
  x2 *= scale;
  y2 *= scale;
  radius *= scale;
  const minX = Math.max(0, Math.floor(Math.min(x1, x2) - radius - 1));
  const maxX = Math.min(img.width - 1, Math.ceil(Math.max(x1, x2) + radius + 1));
  const minY = Math.max(0, Math.floor(Math.min(y1, y2) - radius - 1));
  const maxY = Math.min(img.height - 1, Math.ceil(Math.max(y1, y2) + radius + 1));
  for (let y = minY; y <= maxY; y += 1) {
    for (let x = minX; x <= maxX; x += 1) {
      const distance = distanceToSegment(x + 0.5, y + 0.5, x1, y1, x2, y2);
      const alpha = clamp(radius + 0.5 - distance, 0, 1) * opacity;
      if (alpha > 0) blend(img.pixels, img.width, x, y, color, alpha);
    }
  }
}

function drawTopHighlight(img) {
  const { width, height, pixels } = img;
  const radius = width * 0.235;
  for (let y = 0; y < height * 0.28; y += 1) {
    for (let x = 0; x < width; x += 1) {
      const px = x + 0.5;
      const py = y + 0.5;
      const cx = width / 2;
      const dx = Math.max(Math.abs(px - cx) - (width / 2 - radius), 0);
      const dy = Math.max(Math.abs(py - height / 2) - (height / 2 - radius), 0);
      const dist = Math.hypot(dx, dy) - radius;
      if (dist > 0.5) continue;
      const alpha = (1 - y / (height * 0.28)) * 0.11;
      blend(pixels, width, x, y, hex("#ffffff"), alpha);
    }
  }
}

function flattenPath(commands, scale, segments) {
  const points = [];
  let current = null;
  for (const command of commands) {
    if (command.type === "move") {
      current = { x: command.x * scale, y: command.y * scale };
      points.push(current);
      continue;
    }
    for (let i = 1; i <= segments; i += 1) {
      const t = i / segments;
      points.push(cubic(current, command.c1, command.c2, command.end, t, scale));
    }
    current = { x: command.end.x * scale, y: command.end.y * scale };
  }
  return points;
}

function move(x, y) {
  return { type: "move", x, y };
}

function curve(c1x, c1y, c2x, c2y, x, y) {
  return {
    type: "curve",
    c1: { x: c1x, y: c1y },
    c2: { x: c2x, y: c2y },
    end: { x, y },
  };
}

function translatePath(commands, dx, dy) {
  return commands.map((command) => {
    if (command.type === "move") return move(command.x + dx, command.y + dy);
    return curve(
      command.c1.x + dx,
      command.c1.y + dy,
      command.c2.x + dx,
      command.c2.y + dy,
      command.end.x + dx,
      command.end.y + dy,
    );
  });
}

function cubic(start, c1, c2, end, t, scale) {
  const p0x = start.x;
  const p0y = start.y;
  const p1x = c1.x * scale;
  const p1y = c1.y * scale;
  const p2x = c2.x * scale;
  const p2y = c2.y * scale;
  const p3x = end.x * scale;
  const p3y = end.y * scale;
  const u = 1 - t;
  return {
    x: u * u * u * p0x + 3 * u * u * t * p1x + 3 * u * t * t * p2x + t * t * t * p3x,
    y: u * u * u * p0y + 3 * u * u * t * p1y + 3 * u * t * t * p2y + t * t * t * p3y,
  };
}

function insidePolygon(x, y, polygon) {
  let inside = false;
  for (let i = 0, j = polygon.length - 1; i < polygon.length; j = i, i += 1) {
    const pi = polygon[i];
    const pj = polygon[j];
    const intersects = pi.y > y !== pj.y > y && x < ((pj.x - pi.x) * (y - pi.y)) / (pj.y - pi.y) + pi.x;
    if (intersects) inside = !inside;
  }
  return inside;
}

function bounds(polygon, width, height) {
  let minX = width - 1;
  let minY = height - 1;
  let maxX = 0;
  let maxY = 0;
  for (const point of polygon) {
    minX = Math.min(minX, Math.floor(point.x));
    minY = Math.min(minY, Math.floor(point.y));
    maxX = Math.max(maxX, Math.ceil(point.x));
    maxY = Math.max(maxY, Math.ceil(point.y));
  }
  return {
    minX: clamp(Math.floor(minX), 0, width - 1),
    minY: clamp(Math.floor(minY), 0, height - 1),
    maxX: clamp(Math.ceil(maxX), 0, width - 1),
    maxY: clamp(Math.ceil(maxY), 0, height - 1),
  };
}

function distanceToSegment(px, py, x1, y1, x2, y2) {
  const vx = x2 - x1;
  const vy = y2 - y1;
  const wx = px - x1;
  const wy = py - y1;
  const len2 = vx * vx + vy * vy;
  const t = len2 === 0 ? 0 : clamp((wx * vx + wy * vy) / len2, 0, 1);
  const cx = x1 + t * vx;
  const cy = y1 + t * vy;
  return Math.hypot(px - cx, py - cy);
}

function writePng(path, img) {
  const raw = Buffer.alloc((img.width * 4 + 1) * img.height);
  for (let y = 0; y < img.height; y += 1) {
    const rawOffset = y * (img.width * 4 + 1);
    raw[rawOffset] = 0;
    img.pixels.copy(raw, rawOffset + 1, y * img.width * 4, (y + 1) * img.width * 4);
  }

  const png = Buffer.concat([
    Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]),
    chunk("IHDR", ihdr(img.width, img.height)),
    chunk("IDAT", deflateSync(raw)),
    chunk("IEND", Buffer.alloc(0)),
  ]);
  writeFileSync(path, png);
}

function ihdr(width, height) {
  const buf = Buffer.alloc(13);
  buf.writeUInt32BE(width, 0);
  buf.writeUInt32BE(height, 4);
  buf[8] = 8;
  buf[9] = 6;
  buf[10] = 0;
  buf[11] = 0;
  buf[12] = 0;
  return buf;
}

function chunk(type, data) {
  const typeBuf = Buffer.from(type, "ascii");
  const out = Buffer.alloc(12 + data.length);
  out.writeUInt32BE(data.length, 0);
  typeBuf.copy(out, 4);
  data.copy(out, 8);
  out.writeUInt32BE(crc32(Buffer.concat([typeBuf, data])), 8 + data.length);
  return out;
}

const crcTable = new Uint32Array(256).map((_, i) => {
  let c = i;
  for (let k = 0; k < 8; k += 1) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1;
  return c >>> 0;
});

function crc32(buf) {
  let c = 0xffffffff;
  for (const byte of buf) c = crcTable[(c ^ byte) & 0xff] ^ (c >>> 8);
  return (c ^ 0xffffffff) >>> 0;
}

function blend(pixels, width, x, y, color, alpha) {
  const offset = (y * width + x) * 4;
  const srcA = clamp(alpha, 0, 1);
  const dstA = pixels[offset + 3] / 255;
  const outA = srcA + dstA * (1 - srcA);
  if (outA <= 0) return;
  pixels[offset] = Math.round((color.r * srcA + pixels[offset] * dstA * (1 - srcA)) / outA);
  pixels[offset + 1] = Math.round((color.g * srcA + pixels[offset + 1] * dstA * (1 - srcA)) / outA);
  pixels[offset + 2] = Math.round((color.b * srcA + pixels[offset + 2] * dstA * (1 - srcA)) / outA);
  pixels[offset + 3] = Math.round(outA * 255);
}

function gradient(stops, t) {
  t = clamp(t, 0, 1);
  for (let i = 0; i < stops.length - 1; i += 1) {
    const [fromT, from] = stops[i];
    const [toT, to] = stops[i + 1];
    if (t >= fromT && t <= toT) return mix(from, to, (t - fromT) / (toT - fromT));
  }
  return stops[stops.length - 1][1];
}

function mix(a, b, t) {
  t = clamp(t, 0, 1);
  return {
    r: Math.round(a.r + (b.r - a.r) * t),
    g: Math.round(a.g + (b.g - a.g) * t),
    b: Math.round(a.b + (b.b - a.b) * t),
  };
}

function hex(value) {
  return {
    r: Number.parseInt(value.slice(1, 3), 16),
    g: Number.parseInt(value.slice(3, 5), 16),
    b: Number.parseInt(value.slice(5, 7), 16),
  };
}

function clamp(value, min, max) {
  return Math.min(max, Math.max(min, value));
}

writePng(appIconPath, renderAppIcon(1024));
writePng(trayIconPath, renderTrayIcon(128));
buildIcns();

console.log(`generated ${appIconPath}`);
console.log(`generated ${trayIconPath}`);
console.log(`generated ${icnsPath}`);
