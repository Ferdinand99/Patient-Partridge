import { mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterEach, describe, expect, it } from "vitest";
import { check, install, mismatchMessage, sha256, toolsFolder, verify } from "./linux-tools.mjs";

const dirs = [];
afterEach(() => dirs.splice(0).forEach((d) => rmSync(d, { recursive: true, force: true })));
const folder = () => {
  const d = mkdtempSync(join(tmpdir(), "tools-"));
  dirs.push(d);
  return d;
};

const good = Buffer.from("the pinned tool");
const lock = { tools: [{ file: "tool.AppImage", url: "https://example.com/tool", sha256: sha256(good) }] };

describe("linux tools", () => {
  it("are used only when the checksum is the pinned one", () => {
    expect(() => verify(good, lock.tools[0])).not.toThrow();
    expect(() => verify(Buffer.from("something else"), lock.tools[0])).toThrow(/Checksum mismatch/);
    expect(() => verify(good, { ...lock.tools[0], sha256: "abc" })).toThrow();
  });

  it("are downloaded, checked and written, and a download that does not match is never written", async () => {
    const dir = folder();
    await install(lock, dir, async () => good);
    expect(readFileSync(join(dir, "tool.AppImage"))).toEqual(good);
    const other = folder();
    await expect(install(lock, other, async () => Buffer.from("replaced upstream"))).rejects.toThrow(/Checksum mismatch/);
    expect(() => readFileSync(join(other, "tool.AppImage"))).toThrow();
  });

  it("are left alone when they are there and right, and replaced when they are there and wrong", async () => {
    const dir = folder();
    writeFileSync(join(dir, "tool.AppImage"), good);
    let fetched = 0;
    await install(lock, dir, async () => (fetched++, good));
    expect(fetched).toBe(0);
    writeFileSync(join(dir, "tool.AppImage"), "changed by something");
    await install(lock, dir, async () => (fetched++, good));
    expect(fetched).toBe(1);
    expect(readFileSync(join(dir, "tool.AppImage"))).toEqual(good);
  });

  it("say why, and what to do, when a tool from a rolling release is not the pinned file", () => {
    const rolling = { ...lock.tools[0], rolling: true };
    const text = mismatchMessage(rolling, sha256(Buffer.from("newer")));
    expect(text).toContain("Checksum mismatch for tool.AppImage");
    expect(text).toContain("rolling release");
    expect(text).toContain("scripts/linux-tools.lock.json");
    expect(text).toContain("before the release PR is merged");
    expect(() => verify(Buffer.from("newer"), rolling)).toThrow(/rolling release/);
    // A tool with a fixed address gets the plain message.
    expect(mismatchMessage(lock.tools[0], "0".repeat(64))).not.toContain("rolling");
  });

  it("are compared online without writing anything, and every difference is listed", async () => {
    const two = {
      tools: [
        lock.tools[0],
        { file: "other.AppImage", url: "https://example.com/other", sha256: sha256(good), rolling: true },
        { file: "gone.AppImage", url: "https://example.com/gone", sha256: sha256(good) },
      ],
    };
    const answers = { "https://example.com/tool": good, "https://example.com/other": Buffer.from("replaced") };
    const problems = await check(two, async (url) => {
      if (!(url in answers)) throw new Error(`Download failed: ${url}: 404 Not Found`);
      return answers[url];
    });
    expect(problems).toHaveLength(2);
    expect(problems[0]).toContain("other.AppImage");
    expect(problems[0]).toContain("rolling release");
    expect(problems[1]).toContain("404");
    expect(await check(lock, async () => good)).toEqual([]);
  });

  it("go into the cache folder of Tauri", () => {
    expect(toolsFolder({ XDG_CACHE_HOME: "/c" })).toBe(join("/c", "tauri"));
  });

  it("the lock names the three files the bundler looks for, with full checksums", () => {
    const real = JSON.parse(readFileSync(new URL("./linux-tools.lock.json", import.meta.url), "utf8"));
    expect(real.tools.map((t) => t.file)).toEqual(["AppRun-x86_64", "linuxdeploy-07333c6-x86_64.AppImage", "linuxdeploy-plugin-appimage.AppImage"]);
    // The one that comes from a release called "continuous" is marked, so a mismatch explains itself.
    expect(real.tools.filter((t) => t.rolling).map((t) => t.url)).toEqual([expect.stringContaining("/releases/download/continuous/")]);
    for (const t of real.tools) {
      expect(t.sha256).toMatch(/^[0-9a-f]{64}$/);
      expect(t.url.startsWith("https://github.com/")).toBe(true);
    }
  });
});
