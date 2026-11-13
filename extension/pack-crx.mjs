// Wrap a zipped extension in a signed CRX3 container, the file Chromium
// browsers install. No dependencies.
// usage: node pack-crx.mjs ZIP KEY_PEM OUT_CRX
//
// The extension's ID is derived from the key, so the same key has to sign
// every release or browsers treat an update as a different extension. The key
// is created on first use if the file does not exist.
import { createHash, createPrivateKey, createPublicKey, generateKeyPairSync, sign } from 'node:crypto';
import { existsSync, readFileSync, writeFileSync } from 'node:fs';

const [zipPath, keyPath, outPath] = process.argv.slice(2);
if (!zipPath || !keyPath || !outPath) {
  console.error('usage: node pack-crx.mjs ZIP KEY_PEM OUT_CRX');
  process.exit(2);
}

if (!existsSync(keyPath)) {
  const { privateKey } = generateKeyPairSync('rsa', { modulusLength: 2048 });
  writeFileSync(keyPath, privateKey.export({ type: 'pkcs8', format: 'pem' }), { mode: 0o600 });
  console.error(`created a new signing key at ${keyPath}: keep it, the extension's ID comes from it`);
}

const privateKey = createPrivateKey(readFileSync(keyPath));
if (privateKey.asymmetricKeyType !== 'rsa') {
  console.error(`${keyPath} is not an RSA key`);
  process.exit(1);
}
const publicKey = createPublicKey(privateKey).export({ type: 'spki', format: 'der' });
const zip = readFileSync(zipPath);

// protobuf: a length-delimited field
const varint = (n) => {
  const out = [];
  while (n > 0x7f) { out.push((n & 0x7f) | 0x80); n >>>= 7; }
  out.push(n);
  return Buffer.from(out);
};
const field = (number, bytes) => Buffer.concat([varint((number << 3) | 2), varint(bytes.length), bytes]);
const u32 = (n) => { const b = Buffer.alloc(4); b.writeUInt32LE(n); return b; };

// SignedData { crx_id = 1 }: the first 16 bytes of the key's SHA-256
const crxId = createHash('sha256').update(publicKey).digest().subarray(0, 16);
const signedData = field(1, crxId);

const signature = sign('sha256', Buffer.concat([
  Buffer.from('CRX3 SignedData\0'), u32(signedData.length), signedData, zip,
]), privateKey);

// CrxFileHeader { sha256_with_rsa = 2 { public_key = 1, signature = 2 }, signed_header_data = 10000 }
const header = Buffer.concat([
  field(2, Buffer.concat([field(1, publicKey), field(2, signature)])),
  field(10000, signedData),
]);

writeFileSync(outPath, Buffer.concat([Buffer.from('Cr24'), u32(3), u32(header.length), header, zip]));

// the ID as browsers show it: each hex digit of the key hash, as a..p
const id = [...crxId.toString('hex')].map((c) => String.fromCharCode(97 + parseInt(c, 16))).join('');
console.log(`${outPath}: extension ID ${id}`);
