// Runs the shell text of the "Open or update the issue" step of .github/workflows/pdfium-update.yml
// against a fake `gh`, to prove: one open issue at most, a newer build updates it, a closed issue
// skips its build, and hostile text in the title or body is only ever data.

import { spawnSync } from "node:child_process";
import { chmodSync, existsSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterEach, describe, expect, it } from "vitest";

const workflow = readFileSync(new URL("../.github/workflows/pdfium-update.yml", import.meta.url), "utf8").replace(/\r\n/g, "\n");

function stepScript() {
  const start = workflow.indexOf("      - name: Open or update the issue\n");
  expect(start).toBeGreaterThan(-1);
  const run = workflow.indexOf("        run: |\n", start);
  const lines = workflow.slice(run + "        run: |\n".length).split("\n");
  const body = [];
  for (const line of lines) {
    if (line !== "" && !line.startsWith("          ")) break;
    body.push(line.slice(10));
  }
  return body.join("\n");
}

const tools = ["bash", "jq"].every((t) => spawnSync(t, ["--version"]).status === 0);
const dirs = [];
afterEach(() => dirs.splice(0).forEach((d) => rmSync(d, { recursive: true, force: true })));

const FAKE_GH = `#!/usr/bin/env bash
echo "$*" >> "$GH_LOG"
if [ "$1 $2" = "issue list" ]; then
  state=open; jq=""
  while [ $# -gt 0 ]; do
    case "$1" in --state) state="$2";; --jq) jq="$2";; esac
    shift
  done
  if [ "$state" = open ]; then data="$FAKE_OPEN"; else data="$FAKE_CLOSED"; fi
  if [ -n "$jq" ]; then printf '%s' "$data" | jq -r "$jq"; else printf '%s' "$data"; fi
elif [ "$1 $2" = "issue edit" ] || [ "$1 $2" = "issue create" ]; then
  # Log the title and body arguments exactly as they arrive, one per line.
  if [ "$2" = edit ]; then shift 3; else shift 2; fi
  while [ $# -ge 2 ]; do printf 'ARG %s=%s\\n' "$1" "$2" >> "$GH_LOG"; shift 2; done
fi
`;

function run({ open = [], closed = [], title = "PDFium update available: chromium/8200", body = "body" }) {
  const dir = mkdtempSync(join(tmpdir(), "pdfium-issue-"));
  dirs.push(dir);
  writeFileSync(join(dir, "gh"), FAKE_GH);
  chmodSync(join(dir, "gh"), 0o755);
  writeFileSync(join(dir, "step.sh"), `set -euo pipefail\n${stepScript()}`);
  const log = join(dir, "log");
  writeFileSync(log, "");
  const result = spawnSync("bash", [join(dir, "step.sh")], {
    cwd: dir,
    encoding: "utf8",
    timeout: 20000,
    env: {
      PATH: `${dir}:${process.env.PATH}`,
      GH_LOG: log,
      FAKE_OPEN: JSON.stringify(open),
      FAKE_CLOSED: JSON.stringify(closed),
      TITLE: title,
      BODY: body,
    },
  });
  return { ...result, log: readFileSync(log, "utf8"), dir };
}

describe.skipIf(!tools)("the issue step", () => {
  it("opens one issue when none exists", () => {
    const r = run({});
    expect(r.status).toBe(0);
    expect(r.log).toContain("issue create");
    expect(r.log).toContain("ARG --title=PDFium update available: chromium/8200");
  });
  it("updates the open issue to the newest build and opens no second one", () => {
    const r = run({ open: [{ number: 7, title: "PDFium update available: chromium/8150" }] });
    expect(r.status).toBe(0);
    expect(r.log).toContain("issue edit 7");
    expect(r.log).not.toContain("issue create");
    expect(r.log).toContain("ARG --title=PDFium update available: chromium/8200");
  });
  it("changes nothing when the open issue already names this build", () => {
    const r = run({ open: [{ number: 7, title: "PDFium update available: chromium/8200" }] });
    expect(r.status).toBe(0);
    expect(r.log).not.toContain("issue edit");
    expect(r.log).not.toContain("issue create");
  });
  it("takes the newest of several open issues and ignores unrelated ones", () => {
    const r = run({
      open: [
        { number: 3, title: "PDFium update available: chromium/8100" },
        { number: 9, title: "PDFium update available: chromium/8150" },
        { number: 12, title: "Something else" },
      ],
    });
    expect(r.log).toContain("issue edit 9");
  });
  it("opens nothing for a build whose issue was closed", () => {
    const r = run({ closed: [{ title: "PDFium update available: chromium/8200" }] });
    expect(r.status).toBe(0);
    expect(r.log).not.toContain("issue create");
    expect(r.log).not.toContain("issue edit");
  });
  it("opens an issue for a newer build after an older one was closed", () => {
    const r = run({ closed: [{ title: "PDFium update available: chromium/8150" }] });
    expect(r.log).toContain("issue create");
  });
  it("treats a hostile title and body as data", () => {
    const evil = "x'; touch pwned; echo '$(touch pwned2) `touch pwned3` \"; @everyone";
    const r = run({ title: `PDFium update available: ${evil}`, body: evil });
    expect(r.status).toBe(0);
    expect(r.log).toContain(`ARG --body=${evil}`);
    for (const f of ["pwned", "pwned2", "pwned3"]) expect(existsSync(join(r.dir, f))).toBe(false);
  });
});
