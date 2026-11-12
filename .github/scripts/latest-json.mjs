// Écrit `latest.json`, le fichier que lisent les copies installées pour se
// mettre à jour, à partir des signatures attachées à une release, puis
// l'envoie dans cette release.
//
//   node .github/scripts/latest-json.mjs v0.2.0 [--dry-run]
//
// Demande `gh`, connecté ou avec `GH_TOKEN` ; le dépôt vient de
// `GITHUB_REPOSITORY` ou du dossier courant. Le workflow de release le lance
// une fois, quand tous les installateurs sont envoyés : les builds tournent
// côte à côte et écraseraient les entrées les uns des autres s'ils écrivaient
// chacun le fichier.

import { execFileSync } from "node:child_process";
import { mkdtempSync, readFileSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

// Ce que met à jour chaque fichier signé : l'application cherche
// `{os}-{arch}-{paquet}`, selon la façon dont elle a été installée, puis
// `{os}-{arch}`. `fallback` désigne le paquet qui répond à la clé courte.
const BUNDLES = [
  { pattern: /_x64-setup\.exe$/, target: "windows-x86_64", bundle: "nsis", fallback: true },
  { pattern: /_x64_[A-Za-z-]+\.msi$/, target: "windows-x86_64", bundle: "msi" },
  { pattern: /_aarch64\.app\.tar\.gz$/, target: "darwin-aarch64", bundle: "app", fallback: true },
  { pattern: /_x64\.app\.tar\.gz$/, target: "darwin-x86_64", bundle: "app", fallback: true },
];

const [tag, ...flags] = process.argv.slice(2);
if (!tag) {
  console.error("usage : latest-json.mjs <étiquette> [--dry-run]");
  process.exit(2);
}
const dryRun = flags.includes("--dry-run");

const gh = (...args) => execFileSync("gh", args, { encoding: "utf8" });

const repository =
  process.env.GITHUB_REPOSITORY ?? gh("repo", "view", "--json", "nameWithOwner", "--jq", ".nameWithOwner").trim();
const release = JSON.parse(gh("release", "view", tag, "-R", repository, "--json", "assets,body"));
const names = release.assets.map((asset) => asset.name);

const folder = mkdtempSync(join(tmpdir(), "latest-json-"));
gh("release", "download", tag, "-R", repository, "--pattern", "*.sig", "--dir", folder);

const platforms = {};
for (const name of names) {
  const found = BUNDLES.find((bundle) => bundle.pattern.test(name));
  if (!found) continue;
  if (!names.includes(`${name}.sig`)) throw new Error(`${name} n'a pas de signature dans la release`);
  const entry = {
    signature: readFileSync(join(folder, `${name}.sig`), "utf8"),
    url: `https://github.com/${repository}/releases/download/${tag}/${name}`,
  };
  platforms[`${found.target}-${found.bundle}`] = entry;
  if (found.fallback) platforms[found.target] = entry;
}

// Une release à laquelle il manque une plateforme laisserait ses postes sans
// la mise à jour : ils ne seraient prévenus qu'à la version suivante.
const missing = [...new Set(BUNDLES.map((bundle) => bundle.target))].filter((target) => !platforms[target]);
if (missing.length > 0) throw new Error(`aucun installateur signé dans ${tag} pour ${missing.join(", ")}`);

const manifest = {
  version: tag.replace(/^v/, ""),
  notes: release.body ?? "",
  pub_date: new Date().toISOString(),
  platforms,
};
const file = join(folder, "latest.json");
writeFileSync(file, `${JSON.stringify(manifest, null, 2)}\n`);

console.log(`latest.json pour ${repository} ${tag} :`);
for (const [key, entry] of Object.entries(platforms)) console.log(`  ${key.padEnd(24)} ${entry.url}`);

if (dryRun) {
  console.log(`non envoyé (--dry-run) : ${file}`);
} else {
  gh("release", "upload", tag, file, "-R", repository, "--clobber");
  console.log("envoyé");
}
