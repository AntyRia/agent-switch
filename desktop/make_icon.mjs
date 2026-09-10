// Generates a solid-color 1024x1024 PNG (no external dependencies).
import { deflateSync } from "node:zlib";
import { writeFileSync } from "node:fs";

const SIZE = 1024;
const RGBA = [59, 130, 246, 255]; // solid blue, opaque

// CRC32 (PNG requires IEEE 802.3 CRC32).
const crcTable = new Uint32Array(256);
for (let n = 0; n < 256; n++) {
  let c = n;
  for (let k = 0; k < 8; k++) {
    c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1;
  }
  crcTable[n] = c >>> 0;
}
function crc32(buf) {
  let c = 0xffffffff;
  for (let i = 0; i < buf.length; i++) {
    c = crcTable[(c ^ buf[i]) & 0xff] ^ (c >>> 8);
  }
  return (c ^ 0xffffffff) >>> 0;
}
function chunk(type, data) {
  const len = Buffer.alloc(4);
  len.writeUInt32BE(data.length);
  const body = Buffer.concat([Buffer.from(type, "ascii"), data]);
  const crc = Buffer.alloc(4);
  crc.writeUInt32BE(crc32(body));
  return Buffer.concat([len, body, crc]);
}

// IHDR: width, height, bit depth 8, color type 6 (RGBA), compression 0, filter 0, interlace 0.
const ihdr = Buffer.alloc(13);
ihdr.writeUInt32BE(SIZE, 0);
ihdr.writeUInt32BE(SIZE, 4);
ihdr[8] = 8;
ihdr[9] = 6;

// Raw scanlines: each row starts with filter byte 0.
const row = Buffer.alloc(1 + SIZE * 4);
row[0] = 0;
for (let x = 0; x < SIZE; x++) {
  row[1 + x * 4] = RGBA[0];
  row[2 + x * 4] = RGBA[1];
  row[3 + x * 4] = RGBA[2];
  row[4 + x * 4] = RGBA[3];
}
const raw = Buffer.concat(Array.from({ length: SIZE }, () => row));
const idat = deflateSync(raw, { level: 9 });

const png = Buffer.concat([
  Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]),
  chunk("IHDR", ihdr),
  chunk("IDAT", idat),
  chunk("IEND", Buffer.alloc(0)),
]);
writeFileSync("icon-src.png", png);
console.log("wrote icon-src.png", png.length, "bytes");
