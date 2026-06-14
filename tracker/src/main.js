// Production entry point. esbuild bundles this into the single minified IIFE served
// at /tracker.js; it simply boots the tracker against the live page.
import { init } from "./tracker.js";

init();
