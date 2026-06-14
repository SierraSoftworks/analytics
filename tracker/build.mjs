// Builds the tracking beacon into a single, heavily-minified IIFE artifact that the
// analytics agent embeds and serves at `/tracker.js`. One artifact, no variants.
import { build, context } from "esbuild";
import { statSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const root = dirname(fileURLToPath(import.meta.url));
const watch = process.argv.includes("--watch");

/** @type {import("esbuild").BuildOptions} */
const options = {
  entryPoints: [join(root, "src/main.js")],
  outfile: join(root, "dist/tracker.js"),
  bundle: true,
  format: "iife",
  // A conservative baseline so the beacon runs on the long tail of browsers without
  // a build step on the consuming site.
  target: ["es2017"],
  minify: true,
  legalComments: "none",
  charset: "utf8",
  logLevel: "info",
};

function reportSize() {
  const { size } = statSync(options.outfile);
  console.log(`tracker.js: ${size} bytes (${(size / 1024).toFixed(2)} KiB) minified`);
}

if (watch) {
  const ctx = await context(options);
  await ctx.watch();
  console.log("watching for changes…");
} else {
  await build(options);
  reportSize();
}
