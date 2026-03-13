#!/usr/bin/env node
const { spawnSync } = require("node:child_process");
const fs = require("node:fs");
const path = require("node:path");

function run(cmd, args) {
  const result = spawnSync(cmd, args, { encoding: "utf8" });
  return {
    ok: result.status === 0,
    status: result.status,
    stdout: (result.stdout || "").trim(),
    stderr: (result.stderr || "").trim()
  };
}

function line(label, value) {
  process.stdout.write(`${label}: ${value}\n`);
}

function firstLine(text) {
  return String(text ?? "")
    .split(/\r?\n/g)
    .map((l) => l.trim())
    .filter(Boolean)[0];
}

line("platform", `${process.platform} / ${process.arch}`);
line("node", `${process.execPath} (${process.version})`);

const whichNpm = run("which", ["npm"]);
line("which npm", whichNpm.ok ? whichNpm.stdout : "(not found)");

const npmVersion = run("npm", ["-v"]);
line(
  "npm -v",
  npmVersion.ok ? npmVersion.stdout : `(failed: ${npmVersion.stderr || npmVersion.status})`
);

const npmPrefix = run("npm", ["prefix", "-g"]);
const prefix = npmPrefix.ok ? firstLine(npmPrefix.stdout) : null;
line("npm prefix -g", prefix || `(failed: ${npmPrefix.stderr || npmPrefix.status})`);

const binDir = prefix ? path.join(prefix, "bin") : null;
if (binDir) {
  line("expected bin", binDir);
  const openclawPath = path.join(binDir, "openclaw");
  line("bin/openclaw exists", fs.existsSync(openclawPath) ? "yes" : "no");
}

const whichOpenclaw = run("which", ["openclaw"]);
line("which openclaw", whichOpenclaw.ok ? whichOpenclaw.stdout : "(not found)");

const openclawVersion = run("openclaw", ["--version"]);
line(
  "openclaw --version",
  openclawVersion.ok
    ? openclawVersion.stdout
    : `(failed: ${openclawVersion.stderr || openclawVersion.status})`
);

if (!whichOpenclaw.ok && binDir) {
  process.stdout.write("\nHint:\n");
  process.stdout.write(`- openclaw may be installed but not in PATH.\n`);
  process.stdout.write(`- Try: export PATH="${binDir}:$PATH"\n`);
  process.stdout.write(`- Or reinstall to regenerate links: npm i -g openclaw --force\n`);
}
