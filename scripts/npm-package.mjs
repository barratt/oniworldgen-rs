// Helpers for build-npm.sh / build-npm.ps1.
//   node scripts/npm-package.mjs write TEMPLATE_PACKAGE_JSON OUT_DIR VERSION
//     writes OUT_DIR/package.json from the template with VERSION set.
//   node scripts/npm-package.mjs check OUT_DIR
//     loads the built package and checks worldgen.version() against package.json.
import fs from 'node:fs';
import path from 'node:path';
import url from 'node:url';

const [command, ...args] = process.argv.slice(2);
if (command === 'write') {
  const [template, out, version] = args;
  const pkg = JSON.parse(fs.readFileSync(template, 'utf8'));
  pkg.version = version;
  fs.writeFileSync(path.join(out, 'package.json'), JSON.stringify(pkg, null, 2) + '\n');
} else if (command === 'check') {
  const [dir] = args;
  const mod = await import(url.pathToFileURL(path.resolve(dir, 'index.js')).href);
  await mod.default({ module_or_path: fs.readFileSync(path.resolve(dir, 'worldgen_bg.wasm')) });
  const pkg = JSON.parse(fs.readFileSync(path.join(dir, 'package.json'), 'utf8'));
  const want = `${pkg.gameBuild}+${pkg.version}`;
  const have = mod.worldgen.version();
  if (have !== want) { console.error(`worldgen.version() is ${have}, expected ${want}`); process.exit(1); }
  const map = mod.worldgen.generate('SNDST-A-1-0-0-0');
  if (map.error || map.failure || !map.worlds?.length) { console.error('SNDST-A-1-0-0-0 did not generate:', map.error ?? map.failure); process.exit(1); }
  console.log(`worldgen.version() = ${have}; SNDST-A-1-0-0-0 generates`);
} else {
  console.error('usage: npm-package.mjs write TEMPLATE OUT_DIR VERSION | check OUT_DIR');
  process.exit(2);
}
