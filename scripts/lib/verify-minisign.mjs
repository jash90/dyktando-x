#!/usr/bin/env node
// Verifies a Tauri updater signature (.sig file = base64 of a minisign signature) against
// the base64 public key from tauri.conf.json (plugins.updater.pubkey).
// Usage: node scripts/lib/verify-minisign.mjs <file> <file.sig> <base64-pubkey>
import { createHash, createPublicKey, verify } from "node:crypto";
import { readFileSync } from "node:fs";

const [file, sigFile, pubkeyB64] = process.argv.slice(2);
if (!file || !sigFile || !pubkeyB64) {
  console.error("usage: verify-minisign.mjs <file> <file.sig> <base64-pubkey>");
  process.exit(2);
}

const decodeLines = (b64) => Buffer.from(b64.trim(), "base64").toString("utf8").split("\n");

// Public key: "untrusted comment: ..." + base64("Ed" | key id (8) | ed25519 key (32)).
const pk = Buffer.from(decodeLines(pubkeyB64)[1], "base64");
const keyId = pk.subarray(2, 10);
const publicKey = createPublicKey({
  key: Buffer.concat([Buffer.from("302a300506032b6570032100", "hex"), pk.subarray(10, 42)]),
  format: "der",
  type: "spki",
});

// Signature: untrusted comment, base64(alg | key id | sig), trusted comment, base64(global sig).
const lines = decodeLines(readFileSync(sigFile, "utf8"));
const sigBlob = Buffer.from(lines[1], "base64");
const alg = sigBlob.subarray(0, 2).toString("latin1");
const signature = sigBlob.subarray(10, 74);
const trustedComment = lines[2].replace(/^trusted comment: /, "");
const globalSig = Buffer.from(lines[3], "base64");

const fail = (msg) => {
  console.error(`FAIL ${file}: ${msg}`);
  process.exit(1);
};
if (!sigBlob.subarray(2, 10).equals(keyId)) fail("signed with a different key");

const data = readFileSync(file);
const message = alg === "ED" ? createHash("blake2b512").update(data).digest() : data;
if (!verify(null, message, publicKey, signature)) fail("signature does not match the file");
const globalMessage = Buffer.concat([signature, Buffer.from(trustedComment, "utf8")]);
if (!verify(null, globalMessage, publicKey, globalSig)) fail("trusted comment signature invalid");
console.log(`OK   ${file} (${trustedComment})`);
